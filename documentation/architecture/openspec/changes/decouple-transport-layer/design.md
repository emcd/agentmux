# Design: Transport Layer Decoupling

## Context

Source: `ideas/transport/2` (reviewed by ACP Specialist, Pty Specialist,
Backend Engineer, Frontend Engineer).

The relay delivery worker (`dispatch/worker.rs`) currently has deep ACP
knowledge: respawn state, bootstrap sequence, permission handler registration,
and the `spawn_blocking` dance for the synchronous ACP state machine. Tmux
operations are scattered across `relay/tmux.rs`, `relay/lifecycle.rs`, and
`relay/delivery/quiescence.rs`. UI delivery (`relay/delivery/ui_delivery.rs`)
is already cleanly separated and is excluded from this refactor.

## Goals / Non-Goals

- Goals: clean module boundaries; each transport owns its implementation;
  relay worker dispatches generically; no behavior change in Slices 1–4.
- Non-Goals: async rewrite of ACP delivery; UI transport adoption (Slice 5);
  `do`/`available_commands` transport methods (deferred to `ideas/acp/1`).

## Decisions

### Sync trait methods (not async)

**Decision**: All `Transport` trait methods are synchronous. The relay worker
keeps wrapping calls in `spawn_blocking`.

**Rationale**: The ACP delivery body is deliberately synchronous and moved by
value into `spawn_blocking` (worker.rs:202, 436–460); "sync core never crosses
.await" is an explicit invariant. Making trait methods async would invert that
contract and require each impl to internally manage `spawn_blocking`, moving
the async/sync boundary without benefit. Keeping it sync preserves
`PersistentAcpWorkerRuntime` move-in/move-out semantics intact.

### Enum dispatch (not `Box<dyn Transport>`)

**Decision**: `TransportImpl { Acp(AcpTransport), Tmux(TmuxTransport) }` with
match delegation.

**Rationale**: The `Transport` trait uses `-> impl Future<Output = …> + Send`
style is not needed (sync decision above). But even if it were async, RPITIT
traits are non-object-safe — `Box<dyn Transport>` is impossible without
`async_trait` or explicit boxed futures. The transport set is fixed and small;
enum dispatch has zero heap overhead per call and is idiomatic for this pattern.

### Transport -> relay interactions (supersedes "inbound events: mpsc channel")

The original design routed all three transport->relay interactions through one
`inbound() -> Receiver<TransportEvent>` mpsc channel. That was the wrong
abstraction: none of the three is an async fire-and-forget event. Each now uses
its natural primitive; there is no generic inbound channel, no `TransportEvent`,
no `resolve_permission()`, and no `look()` method on `Transport`.

#### Choices: relay-injected blocking resolver

**Decision**: The relay injects a re-entrant `Chooser`
(`Arc<dyn Fn(ChoiceToMake) -> ChoiceMade + Send + Sync>`) via `StartupContext`.
The transport invokes it inline on its own thread and blocks until the operator
decides.

**Rationale**: A tool-call permission is a blocking request, not an event — the
agent turn cannot progress until the operator answers. Modeling it as an async
event plus a later resolver would force the relay worker to drain a channel
concurrently with its prompt-completion wait, a deadlock-sensitive interleave.
An injected blocking callback preserves the "no progress past a pending choice"
semantics, keeps the choice-queue logic in relay, and creates no
transport->relay back-edge (the transport holds an opaque `Arc<dyn Fn>` typed in
`transports`). The chooser is re-entrant (the relay keys per-request state by
choice id under a mutex + per-request condvar), so multiple in-turn choices are
safe; per-delivery correlation it cannot close over (`message_id`, `pending_max`,
decider sessions) rides in `ChoiceToMake`, sourced from `DeliveryContext`.

**Invariant**: the chooser MUST unblock and return `ChoiceMade::Cancelled` on
relay shutdown or respawn invalidation.

#### Completion: folded into `deliver()`

**Decision**: `deliver()` blocks until each envelope reaches a terminal state
and returns the per-envelope outcome; the worker fans out from the return value.
There is no completion callback into relay statics and no completion event.

**Rationale**: The worker already blocks for completion today
(`wait_for_prompt_complete_blocking`) and already owns the sender fan-out
(`complete_task_outcome`). Returning the terminal outcome from `deliver()`
collapses submit + wait + completion-callback into one synchronous call. This is
non-blocking for the relay: the send RPC still returns `Queued` at enqueue, and
`deliver()` blocks only the per-target worker's `spawn_blocking` thread — exactly
today's wait, relocated. The ACP `on_completion` body (`build_acp_completion_result`,
`note_session_served_successfully`, `set_acp_worker_state`, and the choice-outcome
correlation) moves into `deliver()`'s internal completion path.

**Invariant**: `deliver()` MUST observe relay shutdown and return a
terminal/dropped outcome promptly rather than parking the blocking thread
indefinitely on a wedged turn.

#### Look: concurrent read via a published `OutputView` handle

**Decision**: `give_output()` hands the relay an `Arc<dyn OutputView>` that the
look request path reads concurrently. The worker re-fetches the handle after
every `startup()` (ACP respawn allocates a fresh replay buffer) at
`bootstrap_acp_runtime_on_worker_start` (worker.rs:379) and
`drive_acp_worker_respawn` (worker.rs:568).

**Rationale**: A `look` is a concurrent read of transport-maintained state; it
runs on a separate thread and cannot borrow the worker-owned transport, so a
`Transport` method is uncallable from it and a replay push would still need a
relay-side mirror. The shared handle is the natural seam. The handle owns the
bounded prime-wait (reading the transport's shared readiness signal, waiting up
to `LookMode::prime_timeout`) and returns the ACP freshness metadata, so relay
stays transport-generic.

**Invariant**: a `look` racing a respawn MUST yield stale/unavailable metadata
or a clean `TransportError`, never a panic or a read of the wrong target's state.

### UI transport excluded from enum

**Decision**: `ui_delivery.rs` stays as a separate delivery path (not a
`TransportImpl` variant). Slice 5 is deferred indefinitely.

**Rationale**: `deliver_one_target_ui()` is a stateless free function using a
process-global `STREAM_REGISTRY` (OnceLock static). The registry is
populated/torn down by `serve_connection_frames` — entirely outside the
delivery worker lifecycle. A `UiTransport` struct would have most trait methods
structurally vacuous (no-op startup/shutdown/raw_write, `give_output()` -> None,
no choices). The member-resolution special-case at `payload.rs:51`
(`target_member.is_none() && !task.target_is_ui`) cannot be expressed through
the trait — the relay still must identify UI targets separately. Low ROI.

### quiescence.rs split

**Decision**: Core poll loop moves to `src/tmux/transport.rs`; shared types
(`DeliveryWaitError`, `QuiescenceOptions`) stay in `relay/delivery/`.

**Rationale**: The poll loop is pure tmux behavior. The shared types are used
by generic delivery helpers that remain relay-visible.

## Module Boundaries After Refactor

```
src/transports/
  contract.rs     — Transport trait, TransportImpl enum, shared types

src/acp/          — (already exists; grows)
  transport.rs    — ACP Transport impl (from relay/delivery/acp_delivery.rs)
  state.rs        — (from relay/delivery/acp_state.rs)
  permission.rs   — (from relay/delivery/permission_state.rs)
  observability.rs— (from relay/delivery/observability.rs)
  client.rs       — merged with relay/delivery/acp_client.rs

src/tmux/         — (new module)
  pane.rs         — pane ops + command plumbing (from relay/tmux.rs)
  lifecycle.rs    — session lifecycle primitives (from relay/lifecycle.rs)
  transport.rs    — Tmux Transport impl + quiescence loop

src/relay/delivery/ — (relay-specific only)
  dispatch/worker.rs  — generic loop via TransportImpl
  quiescence.rs       — shared types only (DeliveryWaitError, QuiescenceOptions)
  ui_delivery.rs      — stays as-is
```

## Risks / Trade-offs

- **Slice 2 scope**: resolved by the contract amendment. The original inbound
  channel restructure (with its deadlock-sensitive worker concurrency change) is
  gone; choices use an injected blocking resolver, completion folds into
  `deliver()`, and look uses a published `OutputView` handle. The "re-subscribe"
  step is now a plain re-fetch of the handle.
- **`acp_client.rs` merge**: `src/acp/client.rs` already exists; relay's
  `acp_client.rs` must merge cleanly into it, not overwrite. (Done in Slice 2a.)
