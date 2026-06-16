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

## 2. Slice 2 — Implement Transport for ACP; inbound channel restructure

- [ ] 2.1 Move `relay/delivery/acp_delivery.rs` → `src/acp/transport.rs`;
      implement `Transport` for `AcpTransport`
- [ ] 2.2 Move `relay/delivery/acp_state.rs` → `src/acp/state.rs`
- [ ] 2.3 Move `relay/delivery/permission_state.rs` → `src/acp/permission.rs`
- [ ] 2.4 Move `relay/delivery/observability.rs` → `src/acp/observability.rs`
- [ ] 2.5 Merge `relay/delivery/acp_client.rs` into existing
      `src/acp/client.rs` (do not overwrite)
- [ ] 2.6 Restructure inbound event path: replace callbacks + shared state
      with transport-owned mpsc channel; `inbound()` returns `Some(Receiver)`
- [ ] 2.7 Update `bootstrap_acp_runtime_on_worker_start` (worker.rs:379)
      to re-call `inbound()` and replace stored Receiver after startup
- [ ] 2.8 Update `drive_acp_worker_respawn` (worker.rs:568) same as 2.7
- [ ] 2.9 Worker treats `None` from Receiver as "expected respawn" signal,
      not error — re-subscribes rather than failing
- [ ] 2.10 Add `TransportImpl::Acp` variant; wire into worker dispatch
- [ ] 2.11 Validate: `cargo test` passes; ACP delivery works end-to-end

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
