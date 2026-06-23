## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL dispatch all agent delivery operations
through two non-blocking write methods defined on the `Transport` trait in
`src/transports/contract.rs`:

- `mailw(envelope: DeliveryEnvelope) -> OutcomeFuture` — structured relay message
  write. The relay SHALL populate routing, attribution, message body, timestamp,
  choice-decider, and quiescence fields before calling the transport. The
  transport SHALL enqueue the structured envelope in its internal ordered channel
  and return an outcome future immediately. The transport SHALL render any
  transport-specific representation internally and resolve the future with a
  terminal `SingleDeliveryOutcome` when the write is delivered or reaches a
  terminal failure state. `OutcomeFuture` is
  `oneshot::Receiver<SingleDeliveryOutcome>`: it carries the transport-side
  outcome, not the relay `SendResult`, preserving the transport contract's
  independence from `crate::relay`. The relay worker maps the resolved
  `SingleDeliveryOutcome` onto its `SendResult`.
- `raww(content: String, append_enter: bool) -> OutcomeFuture` — raw input
  write. The transport SHALL enqueue the raw write in its internal ordered
  channel and return an outcome future immediately. `raww` items act as batch
  barriers: the transport SHALL flush any buffered `mailw` items before
  delivering the raw write, maintaining FIFO ordering across both write types.

Each transport type (ACP, Tmux, Ui, and Pubsub when it lands) SHALL implement
these methods in its own module. The relay SHALL dispatch via a `TransportImpl`
enum that delegates without dynamic allocation, and SHALL submit `mailw`/`raww`
uniformly for every target with no transport-type routing fork in the delivery
loop.

`mailw` and `raww` SHALL be the relay's only delivery seam. The relay worker
SHALL NOT pre-render pane-envelope text before calling `mailw`; representation
rendering belongs to the receiving transport. The `DeliveryEnvelope` type SHALL
carry structured message data and per-write control hints, not rendered prompt
text. The legacy synchronous methods — `deliver`, `prepare_delivery`, and
`raw_write` — and the types that existed solely to serve them (`DeliveryContext`,
`DeliveryResult`, `DeliveryPreparation`, `RawWriteResult`) SHALL NOT be retained.

The trait methods SHALL be non-blocking at the relay boundary. The relay delivery
worker runs a concurrent produce-and-collect loop that simultaneously submits new
writes via `mailw`/`raww` and collects resolved outcome futures. The worker SHALL
NOT block on pending futures before submitting new writes. On relay shutdown, the
transport SHALL resolve all pending outcome futures with a `DroppedOnShutdown`
result promptly.

#### Scenario: ACP delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to an ACP target
- **THEN** it calls `TransportImpl::Acp(t).mailw(envelope)` with structured
  message data and receives an outcome future
- **AND** the ACP transport renders pane-envelope text internally, combines
  accumulated rendered envelopes into one turn prompt, submits the turn, and
  resolves the future with the turn outcome

#### Scenario: Tmux delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to a Tmux target
- **THEN** it calls `TransportImpl::Tmux(t).mailw(envelope)` with structured
  message data and receives an outcome future
- **AND** the Tmux transport renders pane-envelope text internally, buffers the
  rendered envelope, waits for pane quiescence using the per-envelope quiescence
  hints, pastes all buffered envelopes, and resolves all pending outcome futures

#### Scenario: UI delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to a `Ui` target
- **THEN** it calls `TransportImpl::Ui(t).mailw(envelope)` with the same
  structured message data used for coder transports
- **AND** the UI transport emits the message as a relay stream event through its
  injected broadcaster closure without parsing pane-envelope text
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the dispatch path

#### Scenario: Concurrent produce loop keeps feeding transport during quiescence wait

- **WHEN** a `mailw` outcome future is pending and new tasks arrive in the relay
  channel
- **THEN** the relay worker submits them via `mailw` without waiting for the
  earlier future to resolve
- **AND** the transport absorbs the new envelopes into its current ordered buffer

#### Scenario: Raww acts as a batch barrier

- **WHEN** the relay calls `raww` after one or more pending `mailw` calls on the
  same transport
- **THEN** the transport flushes the preceding `mailw` batch first
- **AND** then delivers the raw write
- **AND** subsequent `mailw` calls form a new batch

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
- batch-combining or prompt-packing logic,
- pane-envelope rendering for coder transports,
- per-transport `TargetConfiguration::Acp`/`Tmux`/`Ui`/`Pubsub` dispatch arms for
  delivery, nor a relay-internal UI delivery path.

Every target SHALL be transport-delivered: `Ui` and `Pubsub` are first-class
transports (`TransportImpl::Ui`, and `TransportImpl::Pubsub` forward-declared as
a stub like `Pty`), so the relay worker submits `mailw`/`raww` uniformly without a
transport-deliverability capability flag. The only target-type-dependent step is
transport construction.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations, quiescence scheduling, rendering, or session
  lifecycle primitives are present

#### Scenario: UI target delivered through its transport, not a relay path

- **WHEN** the relay receives a delivery task for a `Ui` target
- **THEN** it dispatches `mailw` through `TransportImpl::Ui` uniformly, with no
  transport-type routing fork
- **AND** no `TargetConfiguration::Ui | Pubsub` delivery arm or UI delivery
  short-circuit appears in the dispatch path

## ADDED Requirements

### Requirement: Structured Delivery Message Payload

`DeliveryEnvelope` SHALL carry structured message data sufficient for every
transport to render its own representation without importing `crate::relay` or
parsing already-rendered text.

The structured payload SHALL include:

- `message_id`,
- message body,
- created timestamp,
- namespace,
- canonical sender session id and optional sender display name,
- canonical target session id and optional target display name,
- canonical co-recipient session ids and optional display names,
- authenticated sender identity when available,
- choice decider sessions,
- quiescence hints.

The relay SHALL populate these fields after routing and authorization. Transports
SHALL treat attribution fields as read-only input and SHALL NOT infer or rewrite
sender, target, cc, namespace, or authenticated identity. The namespace SHALL be
the routing namespace used in canonical `session@namespace` identifiers and
out-of-band delivery metadata.

#### Scenario: Relay builds structured payload

- **WHEN** the relay worker accepts a delivery task for any target type
- **THEN** it constructs a `DeliveryEnvelope` containing structured message data
  and per-write control hints
- **AND** it does not render pane-envelope text before calling `mailw`

#### Scenario: Transport consumes relay-authored attribution

- **WHEN** a transport receives a `DeliveryEnvelope`
- **THEN** it uses the relay-populated sender, target, cc, and authenticated
  identity, and namespace fields as authoritative input
- **AND** it does not derive those fields from transport-local state

#### Scenario: UI and coder transports share payload shape

- **WHEN** the same send request targets UI and coder sessions
- **THEN** the relay constructs payloads from the same structured field set
- **AND** UI renders a stream event while coder transports render pane-envelope
  text from those fields
