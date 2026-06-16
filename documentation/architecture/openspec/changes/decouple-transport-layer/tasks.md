## 1. Slice 1 — Define Transport trait and dispatch enum

- [x] 1.1 Create `src/transports/mod.rs` and `src/transports/contract.rs`
- [x] 1.2 Define `Transport` trait with sync methods: `startup`, `deliver`,
      `look`, `is_ready`, `raw_write`, `resolve_permission`, `shutdown`,
      `accept_capacity`, `inbound`
- [x] 1.3 Define `TransportImpl { Acp(AcpTransport), Tmux(TmuxTransport) }`
      enum with match delegation for each method

      Note (from `add-transport-capability-flags`, landed): transport
      capability flags (`can_be_looked`, `can_be_written`,
      `can_stream_output`) already exist as pure derivation methods on
      `SessionType` (`src/configuration/types.rs`), consumed by the look/raww
      capability gates. Incorporate them as first-class methods on each
      `TransportImpl` variant rather than re-deriving from `SessionType`; the
      `Pty` variant activates the forward-declared Pty capability row
      (true/true/true).

      Note (from `rename-grant-to-choose`, landed): `can_give_choices() -> bool`
      also exists on `SessionType` (returns `true` only for `Acp`; `Tmux`,
      `Ui`, and `Pubsub` return `false`). Incorporate it as a first-class
      method on each `TransportImpl` variant alongside the three flags above.
- [x] 1.4 Define supporting types: `StartupContext`, `DeliveryEnvelope`,
      `DeliveryContext`, `DeliveryResult`, `SingleDeliveryOutcome`,
      `TransportEvent`, `TransportStatus`, `TransportReadiness`,
      `RawWriteResult`, `TransportError`, `LookMode`, `LookSnapshotPayload`,
      `PromptReadinessTemplate`, `PermissionResponse`
- [x] 1.5 Register `mod transports;` in `src/lib.rs`
- [x] 1.6 Validate: `cargo check` passes; no behavior change

## 2. Slice 2 — Implement Transport for ACP

Split into 2a (mechanical leaf moves) and 2b (the inbound channel
restructure). Cut by entanglement: `acp_state` and the `acp_client` shim have
no inbound/shared-state coupling; `acp_delivery` + permission extraction + the
channel all rewrite the same inbound surface and must land together.

### Slice 2a — mechanical leaf moves

- [x] 2.2 Move `relay/delivery/acp_state.rs` → `src/acp/state.rs` (wholesale;
      `pub(in crate::relay)` internals widened to `pub(crate)` as move
      mechanics; consumes relay's still-`pub` `AcpWorkerReadinessState` +
      `AcpLookFreshness`/`AcpLookSnapshotSource`)
- [x] 2.5 Delete the `relay/delivery/acp_client.rs` re-export shim and repoint
      its one consumer (`acp_delivery.rs`) at `crate::acp`. The shim only
      re-exported `crate::acp::{AcpStdioClient, PromptCompletion,
      PromptDispatchOutcome}`, which already live in `src/acp/client.rs` — there
      was no content to merge.
- [x] 2.4 (rescoped) Leave `observability.rs` in `relay/delivery/`. It is
      relay-side pub/sub over relay's OWN registries (ACP worker-state watch +
      relay choices-queue broadcast, keyed by the `pub(super)`
      `async_worker::AsyncWorkerKey`, consumed by `choice_state.rs`), not ACP
      wire-protocol code; moving it to `src/acp/` would invert the dependency
      direction. It moves only if `async_worker` moves too (out of scope).

### Slice 2b — ACP Transport impl + inbound channel restructure

- [ ] 2.1 Move `relay/delivery/acp_delivery.rs` → `src/acp/transport.rs`;
      implement `Transport` for `AcpTransport`
- [ ] 2.3 (rescoped) Extract the ACP permission handling — the per-prompt
      `PermissionHandler` closures embedded in `acp_delivery.rs` (around the
      old lines 242 and 542), NOT a standalone `permission_state.rs` (no such
      file exists) — into `src/acp/permission.rs`
- [ ] 2.6 Restructure inbound event path: replace callbacks + shared state
      with transport-owned mpsc channel; `inbound()` returns `Some(Receiver)`
- [ ] 2.7 Update `bootstrap_acp_runtime_on_worker_start` (worker.rs:379)
      to re-call `inbound()` and replace stored Receiver after startup
- [ ] 2.8 Update `drive_acp_worker_respawn` (worker.rs:568) same as 2.7
- [ ] 2.9 Worker treats `None` from Receiver as "expected respawn" signal,
      not error — re-subscribes rather than failing
- [ ] 2.10 Add a third `TransportEvent::DeliveryCompleted` variant so delivery
      completion (today `emit_sender_delivery_outcome_event`, fired from the
      background reader) flows through the single inbound surface alongside
      replay-entries and permission-requests
- [ ] 2.11 Add `TransportImpl::Acp` variant; wire into worker dispatch
- [ ] 2.12 Validate: `cargo test` passes; ACP delivery works end-to-end

## 3. Slice 3 — Implement Transport for Tmux; move Tmux code

- [ ] 3.1 Create `src/tmux/mod.rs`; register `mod tmux;` in `src/lib.rs`
- [ ] 3.2 Move `relay/tmux.rs` → `src/tmux/pane.rs` (pane ops + command
      plumbing)
- [ ] 3.3 Move Tmux lifecycle primitives from `relay/lifecycle.rs` →
      `src/tmux/lifecycle.rs` (session_exists, create_member_once,
      create_member_with_retry, prune_owned_session, list_owned_sessions,
      cleanup_tmux_server_when_unowned, list_all_sessions,
      startup_tmux_member, constants); relay orchestration functions stay
- [ ] 3.4 Move core quiescence loop from `relay/delivery/quiescence.rs` →
      `src/tmux/transport.rs`; leave shared types (DeliveryWaitError,
      QuiescenceOptions) in relay
- [ ] 3.5 Create `src/tmux/transport.rs`; implement `Transport` for
      `TmuxTransport` wrapping pane/lifecycle primitives
- [ ] 3.6 Add `TransportImpl::Tmux` variant; wire into worker dispatch
- [ ] 3.7 Update consumer imports in `handlers.rs`, `dispatch/transport.rs`,
      `lifecycle.rs` to use `crate::tmux::{pane::*, lifecycle::*}`
- [ ] 3.8 Validate: `cargo test` passes; Tmux delivery works end-to-end

## 4. Slice 4 — Remove direct transport imports from relay

- [ ] 4.1 Remove all ACP-specific imports from `relay/delivery/`
- [ ] 4.2 Remove all Tmux-specific imports from `relay/delivery/`
- [ ] 4.3 Confirm `relay/delivery/` contains no direct references to
      `acp_delivery`, `acp_state`, `permission_state`, `relay/tmux`,
      `relay/lifecycle` tmux primitives
- [ ] 4.4 Validate: `cargo test` passes; full integration test suite green
