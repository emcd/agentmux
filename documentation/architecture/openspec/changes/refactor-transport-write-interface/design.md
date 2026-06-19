# Design: Refactor transport write interface

## Context

The relay's per-target delivery worker currently acts as a scheduler for
transport-specific operations: it hoists the Tmux quiescence wait, drains the
channel during the wait (post-quiescence coalesce), pre-combines ACP batches
via `batch_envelopes`, and carries pane identifiers across function boundaries.
This violates the transport-abstraction goal from `decouple-transport-layer`:
relay delivery should be transport-agnostic.

## Goals / Non-Goals

- **Goals**: fully encapsulate quiescence, batching, and prompt-combining in
  each transport; make the relay worker a pure producer; symmetric
  `served_successfully` path for all transports.
- **Non-Goals**: changing the wire protocol, ACP turn model, look/output path,
  `AcpDriverServices` injection, or the `OutputView` interface.

## Decisions

### Decision: non-blocking `mailw`/`raww` write interface

`mailw(envelope) -> OutcomeFuture` and `raww(content, append_enter) ->
OutcomeFuture` return immediately after enqueueing the write in the transport's
internal ordered channel. The relay worker runs a concurrent produce-and-collect
loop using `select!` that simultaneously drains new channel tasks (submitting
each via `mailw`/`raww`) and collects resolved outcome futures. The loop
continues as long as there are pending tasks in the relay channel or unresolved
outcome futures. The relay awaits outcome futures to produce delivery
notification events: fan out `SendResult` to the original caller, call
`note_session_served_successfully`, release pending-slot accounting, and record
delivery inscriptions.

This loop shape preserves the coalesce-during-wait property: while the
transport is internally waiting for quiescence, the relay's `select!` loop
keeps calling `mailw` for newly arrived tasks. The transport's internal ordered
channel absorbs them; when quiescence fires, the transport flushes all
accumulated writes together.

Alternative considered: keep `deliver(Vec<DeliveryEnvelope>)` but move
quiescence inside the transport. Rejected: the relay would still be blocked on
one `deliver()` call and could not submit additional writes during the
transport's internal wait, losing the coalesce-during-wait opportunity without
a compensating mechanism.

### Decision: transport-internal FIFO ordering; raww is a batch barrier

Both `mailw` and `raww` calls are submitted to the transport through a single
ordered internal channel in arrival order. The transport's delivery task
processes them in FIFO order. A `raww` item acts as a batch barrier: when the
delivery task dequeues a `raww`, it first flushes any buffered `mailw` items
(paste the accumulated batch), then delivers the raw write, then continues
accepting new items. This produces natural batch boundaries: contiguous `mailw`
runs are combined into one flush group; each `raww` terminates the preceding
group and begins a new one.

The relay does not need special raww handling — it calls `mailw`/`raww` in
arrival order, and the transport enforces the ordering internally.

### Decision: token cap is transport construction configuration

The maximum-prompt-tokens budget is configured at transport construction time,
not threaded as a per-`mailw` parameter. Each transport that delivers to a
coder/agent harness (ACP, and in principle any future harness transport)
receives this limit at construction and applies it internally during its
combining step. The ACP transport may call `batch_envelopes` from
`crate::envelope` without a relay back-edge. The relay has no visibility into
prompt-size budgeting; the cap is transport-internal.

### Decision: sync-core invariant reversal

`contract.rs` currently documents a deliberate constraint: the transport is a
synchronous, movable state machine; the relay worker owns `spawn_blocking`.
This proposal reverses that decision. Under the new interface, each transport
that needs internal buffering (ACP, Tmux) owns an internal task (or
`spawn_blocking` invocation) and a tokio channel, and the relay↔transport
boundary is future-based rather than synchronous. AcpWorkerDriver's sync state
(runtime handle, bootstrap/respawn lifecycle) must relocate into or be
coordinated with the ACP transport's internal task. The `contract.rs`
constraint comment is to be updated when the trait changes land.

### Decision: quiescence parameters move into the write envelope

`quiet_window` and `quiescence_timeout` move from `DeliveryContext` into
`DeliveryEnvelope` (or a companion quiescence struct on the envelope) so the
transport has per-write quiescence hints without needing `DeliveryContext`
quiescence fields. The relay still receives `quiescence_timeout_ms` from the
send request and attaches it to the task; it just no longer uses it to
orchestrate a barrier itself.

### Decision: ACP transport owns prompt combining and manifest generation

The ACP transport accumulates `mailw` calls in its internal buffer. When it
drains the buffer for a turn, it concatenates the rendered envelope strings into
one combined prompt (with a manifest header if desired) and submits the turn,
respecting its configured token budget (see "token cap is transport construction
configuration" above). `batch_envelopes` and the relay-side token-budget peel
loop are deleted from the relay.

Consequence: the relay no longer defers excess ACP tasks to the carry buffer
via a peel step. Instead, the ACP transport's internal buffer absorbs all
pending writes and combines as many as fit per turn (bounded by token budget);
any remainder is held and submitted in the next turn by the transport's internal
loop.

The ACP internal delivery task also owns the driver lifecycle calls currently
in the relay worker: `driver.mark_busy()` before turn submission,
`driver.mirror_settled_readiness()` after, and
`driver.maybe_respawn_after_delivery(outcome)` keyed off the turn result.

### Decision: transport owns its own delivery task

Each transport implementation that needs internal buffering (ACP, Tmux in the
new model) spawns or owns an internal delivery task or state machine. The relay
worker interacts only through `mailw`/`raww`. For Tmux, the internal task waits
for quiescence, drains whatever `mailw` calls have arrived, then pastes them all.
For ACP, the internal task combines queued `mailw` calls into one turn and
submits; raw writes are treated as batch barriers (see FIFO ordering decision).

### Decision: uniform `served_successfully` from the worker loop

`note_session_served_successfully` is called from `dispatch/worker.rs` for all
transports after their outcome futures resolve, replacing the current split
(Tmux relay-side in `dispatch/transport.rs`, ACP worker-side in `worker.rs`).
`note_tmux_delivered` is deleted. No `TmuxDriverServices` is introduced.

### Decision: ACP driver lifecycle relocates into the ACP transport's internal task

The relay worker today calls `driver.mark_busy()` before delivery,
`driver.mirror_settled_readiness()` after, and
`driver.maybe_respawn_after_delivery(head_reason_code).await` keyed off the
turn outcome. These must move into the ACP transport's internal delivery task
alongside the combining and turn-submission logic, since the relay worker no
longer drives per-turn delivery.

Respawn-invalidation coordination: when the ACP driver triggers a respawn, the
transport's internal task must drain its buffer and resolve all pending outcome
futures with `Cancelled` before `release_runtime()` is called. A
shutdown/respawn-signal channel (e.g. `watch` or `oneshot`) from the driver to
the internal task provides this coordination.

Pending-slot accounting (`reserve_acp_pending_slot`, bound 64) and the
`relay.send.batch_drain.coalesced` inscription shift from the current
per-task worker-loop sites to future resolution in the produce-and-collect loop.
Slot release happens when each outcome future resolves; the inscription records
the batch size from the relay's produce pass.

### Decision: quiescence hint merge semantics

When the Tmux transport flushes a buffer containing multiple envelopes with
differing quiescence hints, it uses the hints from the head (first) envelope of
the current flush group as the effective bounds for the whole group. A later
envelope's `quiescence_timeout_ms` does not extend or shorten a wait already
in progress for the group.

## Risks / Trade-offs

- **Internal transport state machine complexity**: Each transport now owns a
  non-trivial internal buffer + delivery loop. This is offset by the relay
  becoming significantly simpler and the removal of transport-specific relay
  code.
- **Post-quiescence coalesce preserved via concurrent produce loop**: The relay
  worker's `select!` loop keeps calling `mailw` for new tasks while earlier
  outcome futures are pending. This feeds the transport's internal buffer
  during a quiescence wait; when quiescence fires, the transport flushes all
  accumulated writes together.
- **ACP carry-buffer removed**: The relay's ACP peel/carry loop is deleted.
  The ACP transport's internal buffer holds any excess envelopes beyond the
  token budget; the transport submits them in subsequent turns without relay
  involvement. The token cap is enforced via transport construction config
  rather than a relay-side peel step.

## Open Questions

- Should `mailw` accept a timeout for the outcome future (so the relay can
  bound how long it awaits), or is relay shutdown the only terminal signal?
  Current preference: relay shutdown is sufficient; individual write timeouts
  are a transport concern.
