## ADDED Requirements

### Requirement: Pty Prompt Probe and Look Shall Not Block a Tokio Worker Thread

`PtyPromptProbe::observe` and `PtyOutputView::look` SHALL NOT call `blocking_send`/`blocking_recv` on a tokio worker thread. The snapshot handshake through the worker thread’s `mpsc`/`oneshot` channel SHALL be performed off the async runtime.

A `send`/`raww` to a Pty member that triggers a handover-readiness check (`is_ready_for_handover` → `PtyPromptProbe::observe`) SHALL NOT panic with “Cannot block the current thread from within a runtime”.

#### Scenario: Pty send does not panic the tokio worker

- **WHEN** a `send` targets a Pty member and the relay checks handover readiness via `PtyPromptProbe::observe`
- **THEN** the probe completes without panicking the `tokio-runtime-worker` thread
- **AND** the relay logs `relay.send.async.queued` followed by `relay.send.async.completed` (or a structured `runtime_transport_startup_failed` on failure), never a stranded `queued` with no `completed`

> **Note:** `Pty look` uses the same snapshot channel and is fixed by the same `state.rs` change, but is currently unreachable from the relay’s look handler; end-to-end verification is a follow-up.

### Requirement: Held Bundle Deliveries Shall Not Spawn Workers

A delivery to a member of a bundle whose `HostingIntent` is `Hold` SHALL NOT spawn a worker, regardless of transport. The check SHALL be performed once at delivery-group construction, not per-transport, so every future transport inherits it. This closes `issues/runtime/9` (a held bundle stays in the catalog, so `lookup` succeeds, but hold means the relay is deliberately not running it).

#### Scenario: Pty worker is not lazily created by send to a held bundle

- **WHEN** bundle `alpha` is held (`HostingIntent::Hold` — `autostart = false` or `agentmux down alpha`)
- **AND** operator sends `agentmux send --target member@alpha --message wake`
- **THEN** relay does not spawn a Pty child for that member
- **AND** the send resolves as unavailable (held), never as `queued` with a stranded delivery

#### Scenario: Held check is transport-agnostic

- **WHEN** a bundle is held and a delivery targets any member type (Tmux, Pty, ACP, Ui)
- **THEN** the `is_held` check at group construction rejects it before any `build_worker_transport` call, so no transport’s `startup` is invoked for a held target
