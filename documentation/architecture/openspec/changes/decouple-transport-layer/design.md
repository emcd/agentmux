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

### Inbound events: transport-owned mpsc channel

**Decision**: The ACP transport owns an mpsc Sender internally; `inbound()`
hands the Receiver to the relay worker. Tmux returns `None`.

**Rationale**: Transport-owned channels work cleanly with Rust ownership. The
transport creates the channel in `startup`, hands the Receiver to the worker.
Ownership is unambiguous; no cross-boundary Arc needed.

**Hazard**: On respawn, the old transport drops, closing the old Sender; the
old Receiver yields `None`. The worker MUST treat `None` as "expected,
re-subscribe" not "error", and MUST replace the Receiver BEFORE draining. This
is a restructure, not a tweak — the current worker uses callbacks and shared
state for ACP inbound events. Two startup sites need re-subscription:
`bootstrap_acp_runtime_on_worker_start` (worker.rs:379) and
`drive_acp_worker_respawn` (worker.rs:568).

### UI transport excluded from enum

**Decision**: `ui_delivery.rs` stays as a separate delivery path (not a
`TransportImpl` variant). Slice 5 is deferred indefinitely.

**Rationale**: `deliver_one_target_ui()` is a stateless free function using a
process-global `STREAM_REGISTRY` (OnceLock static). The registry is
populated/torn down by `serve_connection_frames` — entirely outside the
delivery worker lifecycle. A `UiTransport` struct would have 5 of 9 trait
methods structurally vacuous (no-op startup/shutdown/raw_write/resolve_permission,
inbound()->None). The member-resolution special-case at `payload.rs:51`
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

- **Slice 2 scope**: inbound channel restructure adds meaningful complexity.
  If the callbacks + shared state model proves hard to untangle, the slice may
  need to be split. Flag early.
- **`acp_client.rs` merge**: `src/acp/client.rs` already exists; relay's
  `acp_client.rs` must merge cleanly into it, not overwrite.
