## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL dispatch all agent delivery operations
through two non-blocking write methods defined on the `Transport` trait in
`src/transports/contract.rs`:

- `mailw(envelope: DeliveryEnvelope) -> OutcomeFuture` — relay-wrapped message
  write. The transport SHALL enqueue the envelope in its internal ordered channel
  and return an outcome future immediately. The transport SHALL resolve the future
  with a terminal `SingleDeliveryOutcome` when the write is delivered or reaches a
  terminal failure state. `OutcomeFuture` is `oneshot::Receiver<SingleDeliveryOutcome>`:
  it carries the transport-side outcome, not the relay `SendResult`, preserving the
  transport contract's independence from `crate::relay`. The relay worker maps the
  resolved `SingleDeliveryOutcome` onto its `SendResult`.
- `raww(content: String, append_enter: bool) -> OutcomeFuture` — raw input
  write. The transport SHALL enqueue the raw write in its internal ordered channel
  and return an outcome future immediately. `raww` items act as batch barriers:
  the transport SHALL flush any buffered `mailw` items before delivering the raw
  write, maintaining FIFO ordering across both write types.

Each transport type (ACP, Tmux, Ui, and Pubsub when it lands) SHALL implement
these methods in its own module. The relay SHALL dispatch via a `TransportImpl`
enum that delegates without dynamic allocation, and SHALL submit `mailw`/`raww`
uniformly for every target with no transport-type routing fork in the delivery
loop.

`mailw` and `raww` SHALL be the relay's only delivery seam. The legacy
synchronous methods they replace — `deliver`, `prepare_delivery`, and
`raw_write` — and the types that existed solely to serve them (`DeliveryContext`,
`DeliveryResult`, `DeliveryPreparation`, `RawWriteResult`) SHALL be removed from
the trait and the contract module; no dead synchronous seam is retained. The
`DeliveryWaitError` type is retained — the Tmux transport's internal quiescence
wait raises it — and `DeliveryEnvelope`/`SingleDeliveryOutcome` remain the
write/outcome vocabulary.

The trait methods SHALL be non-blocking at the relay boundary. The relay
delivery worker runs a concurrent produce-and-collect loop that simultaneously
submits new writes via `mailw`/`raww` and collects resolved outcome futures.
The worker SHALL NOT block on pending futures before submitting new writes:
it uses `select!` (or equivalent) so that writes arriving during a transport's
internal quiescence wait are submitted promptly and absorbed into the
transport's in-progress flush group. The relay awaits outcome futures to fan
out delivery notification events (`SendResult`, `note_session_served_successfully`,
slot release, inscriptions). On relay shutdown, the transport SHALL resolve all
pending outcome futures with a `DroppedOnShutdown` result promptly.

#### Scenario: ACP delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to an ACP target
- **THEN** it calls `TransportImpl::Acp(t).mailw(envelope)` and receives an
  outcome future
- **AND** the `AcpTransport` implementation buffers the envelope internally,
  combines accumulated envelopes into one turn prompt, submits the turn, and
  resolves the future with the turn outcome

#### Scenario: Tmux delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to a Tmux target
- **THEN** it calls `TransportImpl::Tmux(t).mailw(envelope)` and receives an
  outcome future
- **AND** the `TmuxTransport` implementation buffers the envelope, waits for
  pane quiescence using the per-envelope quiescence hints, pastes all buffered
  envelopes, and resolves all pending outcome futures

#### Scenario: UI delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to a `Ui` target
- **THEN** it calls `TransportImpl::Ui(t).mailw(envelope)` like any other
  transport and receives an outcome future
- **AND** the `UiTransport` implementation emits the message as a relay stream
  event through its injected broadcaster closure and resolves the outcome future
  immediately (no quiescence wait, combining, or token budget)
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the dispatch path

#### Scenario: Concurrent produce loop keeps feeding transport during quiescence wait

- **WHEN** a `mailw` outcome future is pending (e.g. Tmux is waiting for
  quiescence) and new tasks arrive in the relay channel
- **THEN** the relay worker submits them via `mailw` without waiting for the
  earlier future to resolve
- **AND** the transport absorbs the new envelopes into its current flush group
- **AND** when quiescence fires, the transport flushes all accumulated envelopes
  together

#### Scenario: raww acts as a batch barrier

- **WHEN** the relay calls `raww` after one or more pending `mailw` calls on
  the same transport
- **THEN** the transport flushes the preceding `mailw` batch first (completing
  quiescence wait and paste if applicable)
- **AND** then delivers the raw write
- **THEN** subsequent `mailw` calls form a new batch

#### Scenario: Three-batch scenario

- **WHEN** the relay submits three envelopes via `mailw`, then one `raww`, then
  two more envelopes via `mailw` to the same transport
- **THEN** the transport produces exactly three delivery groups: the first three
  envelopes combined, the raw write alone, the final two envelopes combined

#### Scenario: Raw write enqueued in FIFO order

- **WHEN** the relay delivers a raw-input task to any transport
- **THEN** it calls `raww(content, append_enter)` and receives an outcome future
- **AND** the transport delivers it in FIFO order after any preceding mailw items
  have been flushed

#### Scenario: Shutdown resolves pending futures

- **WHEN** relay shutdown is requested while outcome futures are pending
- **THEN** each transport resolves all pending futures with `DroppedOnShutdown`
  promptly

### Requirement: Transport Module Boundaries

ACP-specific delivery code SHALL reside in `src/acp/`. Tmux-specific delivery
code SHALL reside in `src/tmux/`. UI stream-broadcast delivery code SHALL reside
in its own transport module (`UiTransport`), not in the relay delivery subsystem.
The relay delivery subsystem SHALL NOT contain transport-specific logic; all
transport dispatch SHALL go through `TransportImpl`. Specifically, the relay
delivery subsystem SHALL NOT contain:
- quiescence scheduling or pane-identifier propagation,
- batch-combining or prompt-packing logic (including `batch_envelopes` and
  token-budget peeling),
- per-transport `TargetConfiguration::Acp`/`Tmux`/`Ui`/`Pubsub` dispatch arms for
  delivery, nor a relay-internal UI delivery path (`deliver_one_target_ui`,
  `should_route_to_ui`).

Every target SHALL be transport-delivered: `Ui` and `Pubsub` are first-class
transports (`TransportImpl::Ui`, and `TransportImpl::Pubsub` forward-declared as
a stub like `Pty`), so the relay worker submits `mailw`/`raww` uniformly without a
transport-deliverability capability flag. The only target-type-dependent step is
transport *construction* (selecting `UiTransport`/`TmuxTransport`/ACP driver from
`session_type()`), which is inherent.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations, quiescence scheduling, or session lifecycle
  primitives are present

#### Scenario: UI target delivered through its transport, not a relay path

- **WHEN** the relay receives a delivery task for a `Ui` target
- **THEN** it dispatches `mailw` through `TransportImpl::Ui` uniformly, with no
  `is_transport_delivered()` flag and no transport-type routing fork
- **AND** no `TargetConfiguration::Ui | Pubsub` delivery arm or
  `deliver_one_target_ui` / `should_route_to_ui` short-circuit appears in the
  dispatch path

### Requirement: Synchronous Delivery Completion

`mailw()` and `raww()` SHALL each return an outcome future that resolves with a
terminal `SingleDeliveryOutcome` when the write reaches a terminal state; the
relay worker maps that outcome onto its `SendResult` (the future carries the
transport-side type, not the relay `SendResult`, preserving the no-relay-dependency
invariant). The relay worker performs sender fan-out by awaiting the returned
futures; there is no transport-issued completion callback or event separate from
the future. The transport SHALL NOT drop a write without resolving its outcome
future. On relay shutdown, all pending futures SHALL resolve with a
dropped/shutdown outcome promptly. This does not block the relay request path: the
send RPC returns `Queued` at enqueue, and outcome futures are awaited only on the
per-target worker.

#### Scenario: mailw future resolves on delivery

- **WHEN** the relay worker calls `mailw(envelope)` on a transport
- **THEN** it receives a future immediately
- **AND** the future resolves with a terminal `SingleDeliveryOutcome` once the
  transport delivers (or fails to deliver) the write, which the relay worker maps
  onto its `SendResult` at the collect site

#### Scenario: Shutdown resolves all pending futures

- **WHEN** relay shutdown is requested while outcome futures are pending
- **THEN** each transport resolves all pending futures with a dropped/shutdown
  `SingleDeliveryOutcome` promptly

## REMOVED Requirements

### Requirement: Pre-Delivery Readiness Barrier

**Reason**: Quiescence gating is now fully encapsulated in each transport's
internal delivery loop. `Transport::prepare_delivery` is removed from the trait.
The relay worker no longer calls a separate prepare step, does not carry a
resolved pane identifier, and does not drain the task channel during a
relay-side barrier. The Tmux transport performs its own quiescence wait
internally, absorbing any `mailw` calls that arrive during the wait into its
buffer before flushing.

**Migration**: Remove all `prepare_delivery` call sites and implementations.
Move quiescence hints (`quiet_window`, `quiescence_timeout`) into
`DeliveryEnvelope`. With `prepare_delivery`, `deliver`, and `raw_write` all
removed, `DeliveryContext` is constructed nowhere, so remove the struct entirely
(rather than only stripping its quiescence/`n_target` fields) along with
`DeliveryResult`, `DeliveryPreparation`, and `RawWriteResult`. Remove
`classify_tmux_quiescence_hoist` and the post-quiescence `extend_batch_with_drain`
call from the worker loop.

### Requirement: Relay-Combined Batch Dispatch

**Reason**: Batch combining and the token-budget peel loop are now
transport-internal. `can_take_batches()` is removed from `SessionType`. The ACP
transport accumulates `mailw` calls and combines them into one turn prompt
internally; any excess is held in the transport's buffer for the next turn
without relay involvement. The relay worker dispatches one write per task and
does not pre-combine.

**Migration**: Delete `batch_envelopes`, the `can_take_batches` flag, and the
relay-side peel/carry loop. The ACP transport acquires its own combining step.
The relay worker loop no longer calls `deliver_non_ui_target_batch`.
