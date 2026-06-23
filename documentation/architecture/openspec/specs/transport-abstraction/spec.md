# transport-abstraction Specification

## Purpose
TBD - created by archiving change decouple-transport-layer. Update Purpose after archive.
## Requirements
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

### Requirement: Choice Resolution via Injected Resolver

The relay SHALL provide each transport a synchronous, re-entrant choice resolver
(`Chooser`) via `StartupContext`. A transport that raises an operator choice
(ACP tool-call permissions) SHALL invoke the resolver inline and block until it
returns, so the agent turn does not progress past a pending choice. The resolver
SHALL carry per-delivery correlation (`message_id`, `target_session`,
`pending_max`, decider sessions) in the `ChoiceToMake` it is given, sourced from
the `DeliveryContext`. There SHALL be no inbound event channel and no
`resolve_permission` method. The resolver SHALL unblock and return
`ChoiceMade::Cancelled` on relay shutdown or respawn invalidation.

#### Scenario: ACP choice blocks the turn until resolved

- **WHEN** an ACP agent raises a tool-call permission request mid-turn
- **THEN** the transport invokes the injected `Chooser` and blocks
- **AND** the agent turn does not complete until the resolver returns a
  `ChoiceMade`

#### Scenario: Chooser cancels on shutdown

- **WHEN** relay shutdown is requested while a choice is pending
- **THEN** the `Chooser` unblocks and returns `ChoiceMade::Cancelled` with a
  shutdown reason code rather than parking the transport thread

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

### Requirement: Concurrent Look via Output View Handle

The relay SHALL obtain a look snapshot through a single polymorphic accessor,
`get_output_view(member, runtime_directory)`, which returns an `OutputView`
handle for any lookable transport and `None` for non-lookable session types. The
relay look handler SHALL NOT branch on transport identity to shape the snapshot;
it SHALL call `OutputView::look(mode)` once on the returned handle.

The accessor SHALL resolve the handle by provenance: a worker-published handle
from the delivery registry when present (ACP today, and any future
worker-backed transport), otherwise a config-constructed handle for transports
whose output is externally addressable. A transport with worker-owned observable
output SHALL publish its handle via `give_output()` through the worker driver's
publish-output hook at bootstrap, and SHALL keep that published handle valid
across respawn by reusing the same transport — republishing only on the fallback
path where the transport is absent — so a `look` racing a respawn never sees a
missing handle; a transport with no worker-owned output (tmux today) SHALL return
`None` from `give_output()`, and its `OutputView` SHALL be constructed by the
accessor from configuration (socket path + session id).

The handle SHALL own the bounded prime-wait (waiting up to
`LookMode::prime_timeout` for a still-initializing target) and SHALL return
transport-neutral freshness metadata (`LookFreshness` / `LookSnapshotSource`) so
the relay remains transport-generic. Each transport's `look()` SHALL validate its
own `LookMode`, returning a `TransportError` for an unsupported parameter
(e.g. tmux returns `validation_offset_unsupported` for `offset > 0`); the relay
SHALL map validation-class transport error codes to relay validation errors. A
`look` racing a respawn SHALL return stale/unavailable metadata or a clean
`TransportError`, never a panic or a read of the wrong target's state.

#### Scenario: Single polymorphic look call

- **WHEN** the relay handles a `look` request for any lookable target
- **THEN** the relay obtains an `OutputView` via `get_output_view` and calls
  `look(mode)` once
- **AND** the look handler contains no `TargetConfiguration::Tmux`/`Acp` arm for
  snapshot shaping

#### Scenario: ACP look reads the worker-published handle

- **WHEN** a `look` request targets an ACP session
- **THEN** the accessor returns the `OutputView` handle published by
  `give_output()`
- **AND** the handle returns the replay snapshot plus freshness metadata without
  borrowing the worker-owned transport

#### Scenario: Tmux look uses a config-constructed view

- **WHEN** a `look` request targets a tmux session, including before any delivery
  has spawned a worker for it
- **THEN** the accessor constructs a `TmuxOutputView` from the socket path and
  session id
- **AND** `look()` returns `LookSnapshotPayload::Lines` from a live pane capture

#### Scenario: ACP handle stays valid across respawn

- **WHEN** the ACP worker driver respawns a dead runtime
- **THEN** the driver reuses the existing transport so its published handle stays
  valid, republishing through the `publish_output` hook only on the fallback path
  where the transport is absent
- **AND** a `look` racing the respawn reads a recovering/stale snapshot through
  the still-valid handle, never a missing handle or the dead buffer

#### Scenario: Tmux rejects unsupported look parameter

- **WHEN** a tmux `look` request carries `offset > 0`
- **THEN** `TmuxOutputView::look` returns a `TransportError` with code
  `validation_offset_unsupported`
- **AND** the relay surfaces it as a validation error, not an internal failure

### Requirement: Transport-Neutral Look Snapshot Vocabulary

The look-snapshot vocabulary SHALL live in the acp-free transport vocabulary
layer (`src/transports/vocabulary.rs`), which SHALL NOT import any concrete
transport module. This vocabulary comprises the structured entry type
(`StructuredEntry`), `ToolCallStatus`, the freshness/source enums (`LookFreshness`,
`LookSnapshotSource`), and the transport-level `LookSnapshotPayload`
(`Lines` | `StructuredEntries`). Concrete transports SHALL produce this
vocabulary rather than define it: `src/acp` SHALL map its `ReplayEntry`
intermediate into `transports::StructuredEntry`, with `ReplayEntry` remaining
ACP-local. No `transports → relay` edge SHALL be introduced.

#### Scenario: Vocabulary layer is concrete-transport-free

- **WHEN** a developer reads `src/transports/vocabulary.rs`
- **THEN** the structured entry type, `ToolCallStatus`, freshness/source enums,
  and transport-level `LookSnapshotPayload` are defined there
- **AND** the module imports no `crate::acp` or `crate::tmux` item

#### Scenario: ACP produces the neutral entry type

- **WHEN** the ACP worker renders a look snapshot
- **THEN** it maps `ReplayEntry` values into `transports::StructuredEntry`
- **AND** the `StructuredEntry` kinds are `user`/`agent`/`cognition`/`invocation`/`update`

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

