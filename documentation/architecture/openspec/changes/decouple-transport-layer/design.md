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

**Behavior delta (Slice 2b, approved)**: folding completion into `deliver()`
drops the dispatch-time `accepted_in_progress` `delivery_outcome` event ACP
emitted the instant a prompt was submitted (before this change, a fire-and-forget
submit returned `delivered_in_progress_result` and the worker emitted it, then the
reader thread's `on_completion` emitted the terminal event). With `deliver()`
synchronous-to-terminal the sender goes `Queued` (send RPC, at enqueue) directly to
the terminal `delivery_outcome`, with no intermediate event. This is the one
intentional behavior change in Slices 1–4; it is acceptable because
`accepted_in_progress` was an artifact of the old fire-and-forget model and is
redundant with the `Queued` the send RPC already returns at enqueue. Preserving it
would require a transport->relay back-edge, which this amendment removes.

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

### Slice 2b execution decisions (boundary details)

Decided while implementing 2b, after tracing the delivery path end to end. These
preserve behavior and keep relay-side scheduling out of the transport:

- **Envelope/prompt boundary — relay pre-combines, relay fans out.** (Refined in
  Slice 4A: the packer stays in relay; the ACP-ness becomes `can_take_batches`.) ACP
  coalescing (`batch_envelopes` token-budget packing + the `accepted_len` peel
  that feeds the worker carry buffer) is relay scheduling and stays in
  `payload.rs`/`orchestration.rs`. The relay hands `deliver()` the already-combined
  prompt as a single `DeliveryEnvelope` and replicates the one returned
  `SingleDeliveryOutcome` across the N coalesced tasks (the worker's
  `complete_task_outcome` loop already supplies each task's own sender identity).
  This mirrors the existing tmux fan-out and avoids moving the packer.
- **Completion captured via an on_completion slot, then blocked on.** `deliver()`
  submits with an `on_completion` that stores the `PromptCompletion` into a shared
  slot, then blocks on `wait_for_prompt_complete` (bounded poll + shutdown gate),
  then builds the terminal outcome inline from the slot + the pending-choice slot.
  On a shutdown break the slot is empty -> a dropped-on-shutdown outcome.
- **Dual readiness, worker-mirrored (forced by decoupling).** (Superseded in
  Slice 4A: the mirroring moves into `src/acp/worker_driver.rs` via an injected
  closure; the relay loop stops touching readiness.) The transport owns
  an `AcpWorkerReadinessState` signal for `is_ready()` and the `OutputView`
  prime-wait (it cannot call relay's `set_acp_worker_state`). The worker mirrors to
  the global `AcpWorkerReadinessState` registry — which stays, because external
  observers (TUI `subscribe_acp_worker_state`) and respawn/startup gating read it.
  The worker sets global `Busy` just before `deliver()` and the terminal state
  after, preserving the observable Busy transition.
- **RawInput routes through `deliver()`.** ACP raww submits text as a prompt and
  blocks to terminal today; routing RawInput tasks through `deliver()` (rendered =
  raw text) preserves that. `raw_write()` is implemented for contract completeness.

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

**Decision (revised in Slice 3, human-directed no-back-edge pass; superseded for
`DeliveryWaitError` in Slice 4A-1)**: The core poll loop moves to
`src/tmux/transport.rs`. `QuiescenceOptions` stays in
`relay/delivery/quiescence.rs`. `DeliveryWaitError` moved WITH the loop to tmux
in Slice 3, then relocated again to `transports::contract` in Slice 4A-1 once it
became the `prepare_delivery` trait return type (see the Slice 4A barrier
section); a trait return type cannot live in a concrete transport without
forcing relay-visible tmux-internal imports.

**Rationale**: The poll loop is pure tmux behavior. `QuiescenceOptions` is
genuinely relay delivery config — the `send`/`raww` handlers construct it, it
rides on the async delivery task, and `ui_delivery` reuses its default timeout —
so it stays relay-side; the loop takes the unpacked primitives
(`quiet_window`, `quiescence_timeout`) instead of the struct. `DeliveryWaitError`
is the loop's return type and is consumed only by the tmux delivery path, so
leaving it in relay while the loop lives in tmux would create exactly the
`tmux -> relay` back-edge this change eliminates; it moves to tmux and relay maps
it to a `SendResult` at the hoist boundary (`dispatch/transport.rs`). This
supersedes the original "leave both in relay" wording, consistent with the
Slice 3 vocabulary relocation (task 3.0) and the `TmuxLifecycleError` boundary
(task 3.3): no transport surfaces a relay-owned type.

### Slice 4A: worker genericization and the pre-delivery barrier

Slice 4A removes the last transport-specific imports from `relay/delivery/` so
the worker dispatches purely through `TransportImpl`. It splits in two: 4A-1
(the barrier) lands first to shrink the surface; 4A-2 (lifecycle relocation +
the batch capability) follows. Guiding principle (human-stated): **relay stays
minimal and routing-focused; per-transport lifecycle lives in the transport.**

#### Pre-delivery barrier: a generic quiescence/readiness gate

**Decision**: Add `Transport::prepare_delivery(&self, ctx: &DeliveryContext) ->
Result<DeliveryPreparation, DeliveryWaitError>`. The worker calls it (on the
blocking pool) before committing the batch; on success it returns a
`DeliveryPreparation` carrying the resolved target as `Option<String>`, which the
worker threads back into `DeliveryContext::pre_resolved_target` for `deliver`.
tmux runs the quiescence poll loop; ACP and pty return immediately for now. This
replaces the worker's direct `crate::tmux::transport::wait_for_quiescent_pane`
call — the tmux-internals reach in `relay/delivery/` (the residual
`TmuxTransport`/`AcpTransport` construction imports there go in 4A-2's worker
genericization, gated by the 4.9 proof-of-absence).

**As built (Slice 4A-1)**: two supporting moves the original decision did not
spell out. (1) `DeliveryContext` grows the quiescence schedule as primitives
(`quiet_window: Duration`, `quiescence_timeout: Option<Duration>`) — it already
carried `pre_resolved_target` and `target_member` (whence prompt-readiness), but
not the schedule the barrier needs; the relay unpacks `QuiescenceOptions` onto
them at the hoist boundary so the transport contract stays below relay. (2)
`DeliveryWaitError` relocates from `crate::tmux::transport` to
`transports::contract` as the trait return type (see the quiescence.rs-split
decision).

**Rationale**: A pre-delivery wait is not a tmux quirk; it is a generic "is the
target ready to receive this dispatch" gate. Pty will poll like tmux, and ACP
can eventually express it as event-stream quiescence (waiting for agent activity
on the events stream to die down) rather than a text-buffer poll. The barrier
stays a distinct relay-side step (not folded into `deliver()`) specifically so
the worker can drain post-wait task arrivals into the batch before paste — the
coalesce-during-wait optimization. Folding it into `deliver()` would lose that,
because the batch is fixed before `deliver()` is called.

**Invariant**: the barrier MUST observe relay shutdown and return promptly; on
timeout/shutdown/unavailability the worker fans the failure template across the
coalesced batch (unchanged from today's hoist).

**Target-handle shape (Pty review)**: `DeliveryPreparation` starts as a string
target — the existing `Option<String>` `pre_resolved_target`. tmux carries the
resolved pane; a transport whose handle is not a string (Pty's fd) re-resolves
cheaply inside its own `deliver()`, the same path tmux already takes when
`pre_resolved_target` is `None`. If we later want the barrier to own resolution
for every transport (preserving "resolved exactly once"), generalize
`DeliveryPreparation` to an opaque per-transport `DeliveryTarget` (enum or boxed
handle). Deferred — not needed until the Pty transport lands. (Pty's quiescence
mechanism will also differ: PTY-fd idle detection rather than tmux capture-pane
polling, making Pty's `prepare_delivery` non-trivial when it lands; the trait
seam is unaffected.)

#### Worker lifecycle relocation (relocate-with-DI)

**Decision**: Move the ACP worker lifecycle —
`bootstrap_acp_runtime_on_worker_start`, `drive_acp_worker_respawn`,
`AcpRespawnState`, and the readiness-registry mirroring — out of
`relay/delivery/dispatch/worker.rs` into `src/acp` as an `AcpWorkerDriver` owned
by `TransportImpl::Acp`. The relay worker loop keeps only the transport-agnostic
skeleton (receive / coalesce / barrier / `deliver()` / pending-accounting /
shutdown). The driver's three relay touchpoints — broadcast UI stream events,
invalidate pending choices on respawn, mirror worker-state into the registry —
are passed in as injected closures, exactly as the `Chooser` is today (`src/acp`
must not import `crate::relay`).

**Rationale**: The respawn machinery (backoff, give-up-after-N init failures,
choice invalidation, ACP-specific stream events) is ACP-domain behavior tmux and
pty never use. Putting a generic respawn framework on the trait would rename ACP
behavior as generic without actually decoupling it — speculative generality.
Relocating it into the transport behind injected closures is the honest seam and
reuses the proven `Chooser` dependency-injection shape. This **supersedes** the
Slice 2b "Dual readiness, worker-mirrored" decision: the worker no longer mirrors
readiness; the driver does, via an injected closure, so the relay loop stops
naming `AcpWorkerReadinessState` entirely. (The registry itself and
`subscribe_acp_worker_state` stay relay-side; see task 4.6 for the naming
follow-up, now that they are honestly ACP-scoped.)

**Invariant**: respawn backoff and re-startup MUST continue to observe
`shutdown_requested()` between sleeps and unblock promptly; the injected
choice-invalidation MUST still fire before each respawn attempt.
(`shutdown_requested()` is in `crate::runtime::signals`, not `crate::relay`, so
the driver calls it directly — no new edge.)

**Construction boundary (ACP + Coordinator review)**: the driver assembles the
relay-provided closures into `StartupContext` (today built by
`prepare_startup_context` in `worker.rs`, carrying `output_view` / `chooser` /
`ready_signal`) during bootstrap and each respawn, then calls
`transport.startup(ctx)` — context construction moves WITH the lifecycle, not
left straddling in relay. Symmetrically, the relay-side site that builds the
three closures imports nothing from `src/acp`: relay closes over its own
services and hands the driver opaque `Arc<dyn Fn>`s, so the dependency arrow is
`relay -> acp` only. Task 4.9's proof-of-absence is the gate.

#### Batch capability flag (`can_take_batches`)

**Decision**: Replace the `target_is_acp` match in
`payload.rs::prepare_batch_delivery_payload` with `SessionType::can_take_batches()`
(ACP → `false`; Tmux/Pty → `true`), exposed as a first-class method on
`TransportImpl` per the capability-flag family (`add-transport-capability-flags`).
Envelope rendering (`render_task_envelope`), token-budget packing
(`batch_envelopes`), the single-batch peel loop, and the `deferred -> carry`
re-queue all stay in relay unchanged.

**Rationale**: The ACP peel exists for one reason — ACP `deliver()` accepts one
prompt batch per turn (one `session/prompt`), while tmux/pty paste all batches.
That is a transport capability, not a relay concern; but the rendering and
packing genuinely are relay/envelope concerns and should not move into the
transport. Reducing the coupling to one capability flag removes the
`TargetConfiguration::Acp` knowledge from `payload.rs` (which has no `crate::acp`
import today — only the config match) without migrating any logic. This refines
the Slice 2b "Envelope/prompt boundary" decision: the packer still stays in
relay; only the ACP-ness becomes a flag.

#### Remove `accept_capacity`

**Decision**: Delete `Transport::accept_capacity()` and its impls.

**Rationale**: It has zero call sites (both impls return `usize::MAX`) and a
static count cannot express ACP's content-dependent single-batch constraint —
that role is filled by `can_take_batches` above. Removing it deletes dead
contract surface rather than leaving a misleading capacity method.

## Module Boundaries After Refactor

```
src/transports/
  contract.rs     — Transport trait, TransportImpl enum, shared types
  vocabulary.rs   — delivery/look vocabulary (SendOutcome, DeliveryPayloadMode,
                    AcpLookFreshness, AcpLookSnapshotSource); relay re-exports
                    for backward-compat (relocated from relay in Slice 3)

src/acp/          — (already exists; grows)
  transport.rs    — ACP Transport impl (from relay/delivery/acp_delivery.rs)
  state.rs        — (from relay/delivery/acp_state.rs)
  permission.rs   — ACP permission handling; extracted from inline PermissionHandler
                    closures in acp_delivery.rs (~old lines 242, 542); wired to
                    the injected Chooser
  worker_driver.rs— ACP bootstrap + respawn lifecycle + readiness mirroring
                    (from relay worker.rs); drives relay services via injected
                    closures, no crate::relay import (Slice 4A-2)
  client.rs       — merged with relay/delivery/acp_client.rs

src/acp/          — stays in relay (NOT moved to src/acp/)
  observability.rs— relay-side pub/sub over relay's own registries
                    (AcpWorkerReadinessState watch + relay choices-queue broadcast,
                    keyed by relay-internal AsyncWorkerKey); moving it would invert
                    the dependency direction

src/tmux/         — (new module)
  pane.rs         — pane ops + command plumbing (from relay/tmux.rs)
  lifecycle.rs    — session lifecycle primitives (from relay/lifecycle.rs)
  transport.rs    — Tmux Transport impl; prepare_delivery barrier wraps the
                    (now module-private) quiescence poll loop. DeliveryWaitError
                    lived here after Slice 3, then moved up to transports::contract
                    in Slice 4A-1 once prepare_delivery became a trait method (its
                    return type); see the quiescence.rs-split decision

src/relay/delivery/ — (relay-specific only)
  dispatch/worker.rs  — transport-agnostic loop skeleton; lifecycle dispatched via
                        TransportImpl (ACP lifecycle now in src/acp/worker_driver.rs)
  dispatch/payload.rs — envelope render + token-budget packing + single-batch peel;
                        ACP-ness reduced to SessionType::can_take_batches() (Slice 4A-2)
  quiescence.rs       — QuiescenceOptions only (DeliveryWaitError now in
                        transports::contract; see quiescence.rs-split decision)
  ui_delivery.rs      — stays as-is
  observability.rs    — relay-side ACP worker state + choices-queue broadcast
```

## Risks / Trade-offs

- **Slice 2 scope**: resolved by the contract amendment. The original inbound
  channel restructure (with its deadlock-sensitive worker concurrency change) is
  gone; choices use an injected blocking resolver, completion folds into
  `deliver()`, and look uses a published `OutputView` handle. The "re-subscribe"
  step is now a plain re-fetch of the handle.
- **`acp_client.rs` merge**: `src/acp/client.rs` already exists; relay's
  `acp_client.rs` must merge cleanly into it, not overwrite. (Done in Slice 2a.)
