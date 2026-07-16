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
code SHALL reside in `src/tmux/`. Pty-specific delivery code SHALL reside in
`src/pty/`. UI stream-broadcast delivery code SHALL reside in its own transport
module (`UiTransport`), not in the relay delivery subsystem. The relay delivery
subsystem SHALL NOT contain transport-specific logic; all transport dispatch SHALL
go through `TransportImpl`. Specifically, the relay delivery subsystem SHALL NOT
contain:

- quiescence scheduling or pane-identifier propagation,
- batch-combining or prompt-packing logic,
- pane-envelope rendering for coder transports,
- per-transport `TargetConfiguration::Acp`/`Tmux`/`Pty`/`Ui`/`Pubsub` dispatch
  arms for delivery, nor a relay-internal UI delivery path.

Every target SHALL be transport-delivered: `Ui` and `Pubsub` are first-class
transports (`TransportImpl::Ui`, and `TransportImpl::Pubsub` forward-declared as
a stub like the prior `Pty` stub), so the relay worker submits `mailw`/`raww`
uniformly without a transport-deliverability capability flag. The only
target-type-dependent step is transport construction.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations, quiescence scheduling, rendering, or session
  lifecycle primitives are present

#### Scenario: Pty code in src/pty/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Pty transport operations, libghostty-vt state access, portable-pty
  I/O, or shared wedge/prime state machine logic are present

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
- sender, target, and co-recipient identities, each carried as a structured
  `AddressIdentity` (canonical `session@namespace` id plus optional display
  name),
- authenticated sender identity when available,
- choice decider sessions,
- quiescence hints.

The payload SHALL carry each party as an `AddressIdentity` value directly; it
SHALL NOT carry a parallel party type whose canonical id is a bare string
requiring per-transport conversion before rendering. Transports SHALL obtain the
bare canonical id via the non-decorating accessor and the decorating header form
via `render_address`.

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

#### Scenario: Payload carries AddressIdentity per party

- **WHEN** the relay constructs the delivery payload
- **THEN** sender, target, and each co-recipient are carried as `AddressIdentity`
  values directly on the payload
- **AND** no transport performs a bare-string-to-identity conversion before
  rendering

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

### Requirement: Worker Readiness Interface

The relay SHALL expose worker readiness through a transport-agnostic interface,
not an ACP-specific one. Any worker-driven transport (ACP today, Pty) that
maintains a multi-state readiness lifecycle SHALL populate the same surface:

- a transport-neutral readiness enum `WorkerReadinessState` with variants
  `Initializing`, `Available`, `Busy`, `Recovering`, and `Unavailable`, carrying
  no ACP-specific naming;
- a per-target registry field (`AsyncWorkerEntry.readiness`) holding one
  `Option<WorkerReadinessState>` — keyed implicitly by the per-target worker key
  `(namespace, runtime_directory, target_session)`, NOT a per-entry
  transport-keyed map, because each target is served by exactly one transport;
- relay-internal mutator/reader `set_worker_readiness` / `get_worker_readiness`;
- an in-process observer `subscribe_worker_readiness` (with publisher
  `publish_worker_readiness`) that yields the current readiness and every
  subsequent transition, and that MAY be subscribed before the worker registers
  and continues to receive transitions after the worker unregisters;
- a public read `read_worker_readiness` returning `Option<&'static str>` with the
  values `initializing` / `available` / `busy` / `recovering` / `unavailable`.

The interface SHALL NOT spell any of these symbols with an `acp`/`Acp` prefix.
Transport-specific readiness *triggers* (e.g. ACP stdin-write and terminal
stopReason mechanics; Pty flush-group dispatch and child exit) SHALL remain in
the owning transport module (`src/acp` and `src/pty` respectively), which drives
the shared interface rather than defining its own readiness type. Tmux does not
drive a multi-state worker readiness lifecycle and is therefore not a populator
of this interface.

#### Scenario: ACP worker populates the shared readiness interface

- **WHEN** the ACP worker transitions readiness (e.g. to `busy` on prompt-write
  success)
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers to `subscribe_worker_readiness` for that target
  observe the transition
- **AND** `read_worker_readiness` returns the corresponding state string

#### Scenario: Pty worker populates the shared readiness interface

- **WHEN** the Pty worker transitions readiness (e.g. to `busy` while a flush
  group is in flight, to `unavailable` on a `pane_wedged` outcome)
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers to `subscribe_worker_readiness` for that target
  observe the transition
- **AND** `read_worker_readiness` returns the corresponding state string

#### Scenario: Readiness surface carries no ACP-specific naming

- **WHEN** a developer reads the worker readiness enum, registry field, observer,
  and public read
- **THEN** none is spelled with an `acp`/`Acp` prefix
- **AND** a second worker-driven transport can populate the same surface without
  introducing a parallel readiness type

#### Scenario: Subscription survives the worker registration window

- **WHEN** a caller subscribes via `subscribe_worker_readiness` before the worker
  for that target is registered
- **THEN** the subscription is established against the per-target publisher
- **AND** the caller observes transitions published once the worker exists and
  after it later unregisters

### Requirement: Three-State Delivery Classifier

Promptable transports that gate delivery on a quiescence wait SHALL classify
each pending flush group, during the quiescence wait for that group, into
one of three terminal states:

- `running` — output is flowing or has settled at the prompt-readiness
  match; the transport continues to wait normally and resolves the flush
  group as `Delivered` when the prompt becomes ready.
- `unresponsive` — during the quiescence wait for the flush group, no
  observable output has been produced within the prime window; the transport
  resolves the flush group as `SendOutcome::Timeout`.
- `wedged` — during the quiescence wait for the flush group, output has
  settled and the prompt-readiness template does not match; the transport
  resolves the flush group as `SendOutcome::Failed` with a transport-defined
  `reason_code` on the same `Failed` variant (for the Tmux transport,
  `reason_code = "pane_wedged"`).

In addition to the three terminal classifications above, the classifier
SHALL recognize a non-terminal **Busy** pre-classification: when the
target's positive terminal-output-write signal (see the `Positive
Activity Signal` requirement) advances between two consecutive
observation polls, the classifier SHALL:

- treat the target as **Busy** for that iteration;
- suppress all terminal classifications for that iteration —
  `running` (Delivered), `unresponsive` (Timeout), AND `wedged`
  (Failed) — regardless of what the readiness-prompt match or the
  inspected-pane-tail emptiness says. While the
  terminal-output-write signal continues to be reported across
  iterations, the classifier SHALL NOT promote the flush group to
  ANY terminal classification;
- reset the consecutive-mismatch counter the wedge classifier uses, so
  any wedged-counter progress accumulated during a prior quiesced period
  is cleared when terminal output resumes;
- emit a `delivery_target_active` diagnostic inscription carrying
  `target_session`, `pane_target` (when the probe surfaces one), and
  `activity_delta` (the magnitude of the activity generation advance).
  The diagnostic dedups by generation: an iteration whose activity
  generation did not advance does not emit a duplicate.

The `Busy` pre-classification SHALL NOT be surfaced as a terminal
classification. The three terminal classifications remain `running`,
`unresponsive`, and `wedged`; `Busy` is the classifier's way of
saying "keep waiting, the target is alive" without committing to a
terminal outcome.

**Scope clarification.** The `Busy` pre-classification is triggered
ONLY by the terminal-output-write signal — that is, bytes being
written to the target's pane/screen. It does NOT trigger when the
target's agent process is busy but is producing zero terminal bytes
(e.g., silent model thinking, pre-output tool-call prep). This
distinction is explicit: a target in silent thinking produces a
constant `activity_generation` value across observations, the
comparator never registers an advance, and the wedge classifier
continues to fire `pane_wedged` on such a target — the same false
positive the change was supposed to prevent. The silent-thinking case
is a real bug but requires a separate process-level aliveness
signal (filed as a follow-up); it is out of scope for this change.

**Branch ordering contract.** The Busy short-circuit SHALL be
evaluated before any terminal-classification branch in the same
observe-sleep-observe iteration. The required branch order in
`quiescence_classify_step` (in `src/transports/quiescence.rs`), after
the second observation capture, is:

1. **Busy short-circuit** — when the activity generation advanced,
   reset the wedge counter, emit `delivery_target_active`, return
   `NeedsWait`.
2. `delivery_ready` check — terminal: returns
   `Done(Ok(snapshot_after.pane_target.unwrap_or_default()))` when
   the snapshot is prompt-ready.
3. Wedge-counter increment block.
4. Wedge check (counter threshold or prime-window elapsed) —
   `Done(Err(Wedged))`.
5. Prime timeout check — `Done(Err(Timeout))`.

This ordering is what implements the Busy-suppresses-all-terminal-
classifications behavior above. In particular, Busy SHALL be
evaluated BEFORE `delivery_ready`; a post-sleep observation that
matches the prompt regex while the activity generation advanced
during the same quiet window SHALL return `NeedsWait` (Busy), not
`Done(Ok(...))` (Delivered). The wedge counter SHALL only advance
during iterations in which the activity signal was also quiesced;
this is an implicit guard from Busy returning early at step 1.

The `unresponsive` and `wedged` classifiers SHALL each be config-surfaced
per the per-transport spec (see `transport-contracts` Tmux Prime Timeout and
Tmux Wedged State Detection requirements for the Tmux surface).

- The Tmux `unresponsive` classifier SHALL be **opt-in**: absent or
  `None` on `[coders.<id>.tmux].prime-timeout-ms` preserves today's
  unbounded behavior.
- The Tmux `wedged` classifier SHALL be **opt-out**: it defaults to
  enabled (`wedge-detection` is `true` when absent or `true`),
  because the cost of a silently-wedged pane is higher than the cost
  of a false-positive wedge. Operators MAY set
  `[coders.<id>.tmux].wedge-detection = false` to preserve the prior
  unbounded-wait behavior.

No operator-observable rendering state on the Tmux transport — copy-mode or
a non-`root` client key-table — SHALL suppress, defer, or otherwise gate any
classification. Such states do not change what `capture-pane` or `cursor_x`
report and do not impede injection (see the `transport-contracts`
`Copy-Mode-Transparent Injection` requirement), so they are not delivery
preconditions. A quiescence wait SHALL always progress toward one of its
terminal classifications; the classifier SHALL NOT hold a flush group in a
non-terminal state on the basis of a rendering signal it cannot bound. (This
does not affect the ACP transport's `pending_choice_outcome` pause, which is a
distinct turn-blocking operator *decision*, not a rendering signal.)

The classifier SHALL be evaluated at the transport's quiescence wait,
NOT at the relay delivery worker. The relay SHALL NOT inspect
`SingleDeliveryOutcome` to make delivery policy decisions; it only
relays the outcome to the MCP/CLI caller and to the diagnostic stream.

The three states are mutually exclusive at the moment of terminal
classification. The classifier SHALL NOT combine them (for example, a
flush group SHALL NOT resolve as `Timeout AND Failed`).

#### Scenario: Tmux delivery classifies into one of three states

- **WHEN** the Tmux transport's quiescence wait observes the target's
  output state during the wait for a flush group
- **THEN** it routes the flush group to exactly one of `Delivered`,
  `Timeout`, or `Failed` with `reason_code = "pane_wedged"`
- **AND** the relay worker treats the resulting `SingleDeliveryOutcome`
  as terminal regardless of which classifier fired

#### Scenario: Tmux wedge detection defaults to enabled

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **THEN** the Tmux transport classifies a settled, non-prompt-ready pane as
  `wedged`
- **AND** resolves the flush group as `Failed` with
  `reason_code = "pane_wedged"`

#### Scenario: Tmux wedge detection opt-out preserves prior behavior

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`
  (if relay shutdown is requested)

#### Scenario: Tmux prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not fire `Timeout` for unresponsive
  targets regardless of how long output remains absent
- **AND** the only terminal failure modes for the flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default) and `Shutdown`

#### Scenario: Classification is unaffected by operator copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the operator
  scrolled it with the mouse wheel)
- **THEN** the classifier evaluates prompt-readiness against the pane's live
  content, which copy-mode does not alter
- **AND** a prompt-ready pane resolves as `Delivered`
- **AND** the transport does NOT suppress or defer classification on account
  of the copy-mode state

#### Scenario: Group atomicity on failure classification

- **WHEN** the Tmux transport's quiescence wait classifies the flush
  group as `unresponsive` or `wedged`
- **THEN** every sender in the flush group receives the same terminal
  outcome
- **AND** the transport does NOT classify individual envelopes
  independently within the same flush group

#### Scenario: Busy short-circuit suppresses wedged classification on active target

- **WHEN** the Tmux transport's quiescence wait observes the target's
  activity signal advancing between two consecutive observation polls
- **AND** the inspected pane tail does not match the prompt-readiness
  template (the screen has not yet returned to the prompt because the
  target is mid-generation)
- **THEN** the transport does NOT classify the flush group as `wedged`
- **AND** continues to wait for either the activity to settle and the
  pane to become prompt-ready, or the prime window to elapse with no
  activity observed
- **AND** emits a `delivery_target_active` diagnostic inscription
  carrying the activity delta

#### Scenario: Pty busy short-circuit suppresses wedged classification on active target

- **WHEN** the Pty transport's quiescence wait observes the worker's
  `last_change_atomic` advancing between two consecutive observation
  polls (new bytes were applied to the libghostty-vt terminal)
- **AND** the inspected screen tail does not match the prompt-readiness
  template
- **THEN** the transport does NOT classify the flush group as `wedged`
- **AND** continues to wait for either the activity to settle and the
  screen to become prompt-ready, or the prime window to elapse with no
  activity observed

#### Scenario: Busy short-circuit resets wedge counter

- **WHEN** the wedge counter has accumulated to one or two consecutive
  identical quiesced-mismatch signatures
- **AND** the next observation reports an activity-signal advance
- **THEN** the wedge counter is reset to zero
- **AND** the counter starts accumulating again only after the activity
  signal quiesces and the pane content remains settled at a non-prompt
  state

#### Scenario: Busy short-circuit defers Delivered during active output (branch-ordering contract)

- **WHEN** the post-sleep observation matches the prompt-readiness
  template (the snapshot would normally resolve the wait as
  `Done(Ok(...))` via the `delivery_ready` branch)
- **AND** the activity generation advanced between the two consecutive
  observation polls
- **THEN** the classifier fires the `Busy` short-circuit (returns
  `NeedsWait`) rather than the `delivery_ready` branch
- **AND** the wedge counter is reset to zero
- **AND** the wait function continues to the next iteration, where
  the `delivery_ready` check will resume only after the activity
  generation has settled AND the snapshot continues to match the
  prompt-readiness template across a consecutive observation pair
- **AND** the classifier does NOT promote the flush group to
  `Delivered` while activity is being reported, even momentarily
  when the snapshot happens to match the prompt regex

This scenario exists to make the branch-ordering contract testable
in `tests/unit/tmux_transport.rs` and `tests/unit/pty_transport.rs`:
a probe that advances `activity_generation` between observations
while keeping `is_prompt_ready == true` MUST resolve as
`QuiescenceAction::NeedsWait`, NOT `Ok(pane)`, until the activity
generation quiesces across an observation pair.

### Requirement: Prime Timeout Envelope Field

The relay SHALL communicate a per-write prime-timeout bound to transports
via a generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field.
The field SHALL be transport-neutral — the relay populates it from
per-coder config without knowing which transport will consume it, and
each transport that performs a prime wait MAY read it or ignore it.

For Tmux-backed sessions, the relay populates
`DeliveryEnvelope.prime_timeout_ms` from
`[coders.<id>.tmux].prime-timeout-ms`. The ACP delivery-side timeout
follow-up will populate the same field for ACP sessions from
`[coders.<id>.acp].prime-timeout-ms` (or a parallel per-coder key under
the ACP table).

The field SHALL replace any prior transport-specific prime-timeout
field shape. The relay SHALL NOT add per-transport timeout fields to
`DeliveryEnvelope` — keeping the envelope transport-neutral preserves
the decoupling arc.

#### Scenario: Tmux prime timeout rides on the generic envelope field

- **WHEN** a Tmux-backed session has
  `[coders.<id>.tmux].prime-timeout-ms` set to a finite millisecond
  value
- **THEN** the relay populates `DeliveryEnvelope.prime_timeout_ms`
  with that value at envelope construction time
- **AND** the Tmux transport reads `prime_timeout_ms` to bound the
  prime window

#### Scenario: ACP follow-up consumes the same generic field

- **WHEN** the ACP delivery-side timeout follow-up lands
- **THEN** it populates `DeliveryEnvelope.prime_timeout_ms` from its
  own per-coder config key for ACP sessions
- **AND** does NOT introduce a transport-prefixed envelope field
  (e.g. `acp_prime_timeout_ms`)

#### Scenario: Transports ignore the field when not relevant

- **WHEN** a transport does not perform a prime wait (e.g. UI today)
- **THEN** it ignores `DeliveryEnvelope.prime_timeout_ms`
- **AND** the relay still populates the field with the configured
  value (the relay does not gate the population on transport type)

### Requirement: Transport-Internal Probe Seam for Testability

Each promptable transport that owns a quiescence wait SHALL expose an
internal probe trait that lets tests inject deterministic quiescence and
prompt-readiness results. The probe trait SHALL be transport-internal (not
part of the `Transport` contract) and SHALL NOT appear in
`src/transports/contract.rs`.

The probe trait SHALL return the next observation on demand so tests can
drive the classifier through specific sequences. The probe SHALL cover
at minimum the four canonical sequences: unresponsive, wedged,
slow-prompt, and normal-flow.

#### Scenario: Tmux probe trait is transport-internal

- **WHEN** a developer reads `src/tmux/transport.rs`
- **THEN** they find a `PaneQuiescenceProbe` trait used by
  `wait_for_quiescent_pane`
- **AND** the trait is not re-exported from `src/transports/`
- **AND** the `Transport` trait in `src/transports/contract.rs` has no
  knowledge of probes

#### Scenario: Tmux unit tests cover the four canonical sequences

- **WHEN** `cargo test --test tmux_transport` runs
- **THEN** it asserts the four canonical probe sequences produce the
  expected terminal outcomes:
  - `AlwaysUnresponsiveProbe` → `SendOutcome::Timeout`
  - `AlwaysWedgeProbe` → `SendOutcome::Failed` +
    `reason_code = "pane_wedged"`
  - `SlowPromptProbe` → `Delivered` after several quiescence ticks
  - `NormalFlowProbe` → `Delivered` without prime or wedge firing

#### Scenario: Tmux unit tests cover wedge default-on and opt-out

- **WHEN** `cargo test --test tmux_transport` runs
- **THEN** a test asserts the wedge classifier fires by default when
  `[coders.<id>.tmux].wedge-detection` is absent
- **AND** a test asserts the wedge classifier does NOT fire when
  `[coders.<id>.tmux].wedge-detection = false`

### Requirement: ACP Prime Timeout Envelope Field Consumption

The ACP transport SHALL consume the generic
`DeliveryEnvelope.prime_timeout_ms: Option<u64>` field on the
envelope it receives via `mailw` / `raww`. The transport SHALL treat
`None` as unbounded (preserving today's behavior); it SHALL treat
`Some(ms)` as the prime window bound for the per-turn wait. The ACP
transport SHALL NOT introduce a transport-prefixed envelope field on
top of the generic `prime_timeout_ms` field.

The prime timer anchor SHALL be "delivery task perspective" — the
moment the ACP transport's internal delivery task first enters the
per-turn `wait_for_prompt_complete` poll, NOT the moment the relay
enqueues the task. The prime timer SHALL NOT reset on
coalesce-during-wait; absorbed envelopes inherit the head envelope's
prime timer anchor.

The prime timer SHALL NOT fire while a `pending_choice_outcome` is
in flight (an operator decision is pending). The transport SHALL
continue to wait without firing the prime timer until the choice
resolves or the turn completes.

On prime-timer fire, the transport SHALL resolve the flush group
with `SendOutcome::Timeout` and `reason_code = "acp_turn_timeout"`,
latch the per-target readiness to `Unavailable`, and signal
respawn-needed through the same path used for
`PromptCompletion::ConnectionClosed`. The transport SHALL NOT inject
further messages into the wedge.

#### Scenario: ACP transport consumes the generic prime timeout field

- **WHEN** an ACP target receives a `DeliveryEnvelope` with
  `prime_timeout_ms = Some(ms)`
- **THEN** the ACP transport reads the field and uses it as the
  prime window bound for the per-turn wait
- **AND** the transport does NOT introduce a separate
  `acp_prime_timeout_ms` envelope field

#### Scenario: ACP transport ignores prime timeout when None

- **WHEN** an ACP target receives a `DeliveryEnvelope` with
  `prime_timeout_ms = None`
- **THEN** the ACP transport preserves today's unbounded behavior
- **AND** the only terminal failure modes are the existing
  `ACP Stop-Reason Outcome Mapping` outcomes and `DroppedOnShutdown`

#### Scenario: ACP transport resolves flush group on prime timer fire

- **WHEN** the ACP transport's prime timer fires for a flush group
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout` with `reason_code = "acp_turn_timeout"`
- **AND** the per-target readiness is latched to `Unavailable`
- **AND** the respawn-needed signal is raised so the worker's
  `check_respawn_needed()` returns `true`
- **AND** a `delivery_prime_timeout` inscription is emitted with
  `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`

### Requirement: Positive Activity Signal

Each promptable transport that owns a quiescence wait SHALL populate the
cross-transport `WedgeObservation.activity_generation` field on every
call to `WedgeProbe::observe` from a transport-native
**terminal-output-write** primitive. The classifier compares this
field across two consecutive observations to detect "did bytes flow
between these two polls" independently of whether the captured
pane/screen content visibly changed. The activity generation is a
monotonic `u64`; the classifier treats an advance as a positive
"terminal-output-write" signal.

**Scope (terminal-output-write, not process-busy):** the field carries
a marker of "bytes being written to the target's terminal." It does
NOT carry a marker of "the target's agent is busy regardless of byte
output" — that is a separate problem requiring a process-level
aliveness signal (filed as a follow-up). A target whose agent is in
silent thinking with zero terminal bytes will populate this field
with a constant value, and the wedge classifier will continue to
fire on it as before.

The transport-native activity primitive SHALL be:

- **Tmux**: `#{window_activity}` (the same primitive
  `RealPaneQuiescenceProbe::wait_for_change` already polls). The Tmux
  probe resolves the marker at observation time and parses it as a
  `u64` epoch-seconds value. When `#{window_activity}` is unavailable
  on the running tmux version (the existing
  `resolve_window_activity_marker` returns `Ok(None)` for unknown /
  invalid / bad format errors), the field SHALL be populated with `0`,
  falling back to pre-change behavior for older tmux versions.

- **Pty**: `last_change_atomic` on `PtyShared` (the `Arc<AtomicU64>`
  the worker thread advances after each `vt_write` batch). The Pty
  probe loads the atomic with `Ordering::Acquire`. The field is
  already `u64`; no parsing needed.

The activity signal SHALL be transport-internal: the field is part of
the cross-transport `WedgeObservation` type but does NOT appear in
`DeliveryEnvelope`, `SingleDeliveryOutcome`, or any relay-facing API.
A transport that does not track activity (or whose primitive is
unavailable) populates the field with a constant (`0`), which falls
back to the pre-change behavior for that transport.

#### Scenario: Tmux probe populates activity_generation from window_activity

- **WHEN** the Tmux `TmuxAsWedgeProbe::observe` is called and
  `#{window_activity}` returns a non-empty value
- **THEN** the resulting `WedgeObservation.activity_generation` is the
  parsed `u64` epoch-seconds value of that marker

#### Scenario: Tmux probe falls back to 0 when window_activity is unavailable

- **WHEN** the Tmux `TmuxAsWedgeProbe::observe` is called and
  `#{window_activity}` is unavailable on the running tmux version
- **THEN** the resulting `WedgeObservation.activity_generation` is `0`
- **AND** the classifier's Busy short-circuit never fires for this
  probe (no activity advance is possible when the field is always `0`)

#### Scenario: Pty probe populates activity_generation from last_change_atomic

- **WHEN** the Pty `PtyQuiescenceProbe::observe` or
  `WorkerTerminalProbe::observe` is called
- **THEN** the resulting `WedgeObservation.activity_generation` is the
  current value of `last_change_atomic` loaded with `Ordering::Acquire`

#### Scenario: Classifier compares activity_generation between observations

- **WHEN** two consecutive `WedgeObservation` snapshots have different
  `activity_generation` values
- **THEN** the classifier recognizes the second observation as
  reporting activity since the first
- **AND** enters the Busy pre-classification for that iteration

### Requirement: Pty Transport Implementation

The system SHALL provide a `PtyTransport` in `src/pty/transport.rs` that
implements the existing `Transport` trait and is wired into
`TransportImpl::Pty(PtyTransport)`. The transport SHALL own one
`libghostty_vt::Terminal<'static, 'static>`, one `portable_pty` master, one
reader thread, and one delivery task. Because all `libghostty_vt` types are
`!Send + !Sync`, the terminal SHALL live on the delivery thread and be reached
from other threads (the relay worker for `mailw`/`raww` dispatch is
non-blocking — the transport enqueues onto an internal `mpsc::Sender`; the look
path is the only direct cross-thread accessor) through a `SnapshotRequest`
channel whose receiver lives on the worker thread. The reader thread feeds
PTY output bytes through `bytes_tx` into the worker, which applies them to
the terminal and advances the `last_change_atomic` shared with
`PtyQuiescenceProbe`.

#### Scenario: Pty startup spawns the child PTY and installs effect handlers

- **WHEN** the relay calls `TransportImpl::Pty(t).startup(context)` for a
  Pty-backed bundle member
- **THEN** the transport opens a `portable_pty` master sized to the per-coder
  `cols` and `rows`
- **AND** spawns the configured child command with `COLORTERM=truecolor` and
  a `TERM` env-var value derived from the per-coder `term-protocol` field
  (defaulting to `xterm-256color` when `term-protocol` is unset; see
  `pty-terminal-protocols` for the configurable `term-protocol` surface)
- **AND** constructs a `libghostty_vt::Terminal` with the same dimensions and
  installs the canonical effect handlers (`on_pty_write`, `on_size`,
  `on_device_attributes`, `on_xtversion`, `on_title_changed`)
- **AND** spawns the reader thread and the delivery task
- **AND** the worker thread publishes `WorkerReadinessState::Available`
  AFTER successful `Terminal::new` + handler installation, then signals the
  init handshake so `startup_inner` returns `TransportStatus::Ready`
- **AND** if `Terminal::new` fails, the worker signals the init handshake
  with the error and `startup_inner` returns `TransportError` (the relay-side
  guard then publishes `WorkerReadinessState::Unavailable`)

#### Scenario: Pty mailw enqueues and resolves via delivery task

- **WHEN** the relay calls `TransportImpl::Pty(t).mailw(envelope)` for a
  Pty-backed target
- **THEN** the transport enqueues the envelope on its internal `mpsc::Sender`
  and returns an `OutcomeFuture` immediately
- **AND** the delivery task picks up the envelope, renders the pane-envelope
  text via `DeliveryMessage::render_pane_envelope`, writes the bytes to the
  PTY master, drains `on_pty_write` responses back to the master, waits for
  quiescence via the shared wedge/prime state machine, and resolves the
  outcome future with the corresponding `SingleDeliveryOutcome`

#### Scenario: Pty look renders formatter text + cursor via snapshot channel

- **WHEN** the relay calls `OutputView::look` on the `PtyOutputView` returned
  by `give_output`
- **THEN** the look implementation sends a `SnapshotRequest` through the
  `snapshot_tx` channel; the worker thread receives it, recreates the
  formatter from `&terminal`, calls `format_alloc(Format::Plain)`, splits the
  result on `\n`, takes the last `mode.lines.unwrap_or(40)` rows, and reads
  the cursor position via `terminal.cursor_x()` / `terminal.cursor_y()`;
  replies on the oneshot with a `SnapshotResponse` carrying the rendered tail
  and cursor coordinates
- **AND** the look implementation returns
  `LookSnapshotPayload::Lines { snapshot_lines }`

#### Scenario: Pty shutdown kills the child before joining transport threads

- **WHEN** the relay calls `TransportImpl::Pty(t).shutdown()`
- **THEN** the transport publishes `WorkerReadinessState::Unavailable`
- **AND** sets `shutdown_flag = true` so the worker thread observes
  the shutdown on its next loop iteration and exits cleanly
- **AND** calls `child.kill()` followed by `child.wait()` on the child
  handle the transport itself holds, BEFORE joining the reader
  thread or the worker thread
- **AND** joins the reader thread handle
- **AND** joins the worker thread handle
- **AND** clears the relay's reference handles (`write_tx`,
  `bytes_tx`, child / reader / worker thread handles); the
  transport itself owns `self.shared` until the `PtyTransport`
  is dropped
- **AND** for a direct long-running silent child (one that did
  not spawn descendants inheriting the PTY slave fd), `shutdown`
  completes without waiting for the child's natural exit — the
  child is killed before any join. The regression test
  `pty_transport_shutdown_returns_within_bound_for_live_silent_child`
  asserts this direct-child case (5 s upper bound with a strict
  < 1 s expectation) as evidence of the implemented behavior.

> **Spec-alignment note (2026-07-16):** the live contract is
> deliberately scoped to the direct-child path the implementation
> controls. The `child.kill()` + `child.wait()` sequence is
> sequenced BEFORE the reader + worker joins, so the direct-child
> case completes without waiting for the child's natural exit.
> The implementation does NOT enforce a universal bound: a child
> that spawned descendants inheriting the PTY slave fd (or any
> external process keeping the slave open) is outside the
> transport's control and is not part of the contract. The
> `pty_transport_shutdown_returns_within_bound_for_live_silent_child`
> test exercises the direct-child path; descendant-held slave fds
> are outside the bounded guarantee.

### Requirement: Generalized Wedge/Prime State Machine

The system SHALL provide a transport-agnostic wedge detection and prime
timeout state machine in `src/transports/quiescence.rs`, shared by all
promptable transports (Tmux, Pty). The state machine SHALL operate over
a `WedgeProbe` trait that exposes a single-snapshot observation shape:

- `observe(&mut self) -> Result<WedgeObservation, String>` — captures
  the probe's current state as a single snapshot. The state machine
  calls this twice per quiescence iteration (before and after the
  `wait_for_change` round). Implementations read any underlying IPC /
  state once and return a consistent snapshot.
- `wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>`
  — blocks until the next `observe()` call would differ from the
  previous one, or the supplied `deadline` elapses. Returns `Ok(())`
  on observed change; `Err(DeliveryWaitError::Timeout)` on deadline
  elapsed with no change; `Err(DeliveryWaitError::Failed)` on probe
  errors. The state machine passes a `deadline` derived from the
  per-coder `prime_timeout_ms` so the probe honors the same prime
  window the loop tracks.

The single-snapshot shape is intentional: a multi-method trait
would do 4-8x more work per iteration when the probe side-effects
on each call, and the existing probe test fixtures'
`abort_after_calls` counters would trip prematurely. The 16-probe
test surface in `tests/unit/tmux_transport.rs` uses this two-method
shape and preserves its `next_evaluation` cadence.

The `WedgeObservation` snapshot SHALL carry these fields (consistent
across all transports; per-transport probes populate them from their
native primitives):

- `inspected_tail: String` — the last `inspect_lines` rows formatted
  for prompt-readiness matching. Empty / whitespace-only indicates
  an empty pane (Unresponsive territory); non-empty + not
  prompt-ready indicates a wedge-class mismatch (Wedged territory).
- `is_prompt_ready: bool` — whether the target is currently
  prompt-ready. The state machine's `running` branch returns `Ok`
  when this is `true`.
- `pane_target: Option<String>` — active pane id (e.g. Tmux `%0`)
  for diagnostic inscriptions. `None` when the probe does not
  surface a pane target (e.g. Pty, which has no tmux-style pane
  id); the state machine omits the field from diagnostics in that
  case.
- `mismatch: Option<ReadinessMismatch>` — readiness-mismatch
  metadata when `is_prompt_ready = false`. The state machine uses
  `mismatch.reason` for the wedge/prime-timeout `reason` payload,
  falling back to deriving a generic reason from the inspected tail
  when `None`.
- `activity_generation: u64` — terminal-output-write marker
  populated at observation time. Tmux probes read
  `#{window_activity}` parsed as a `u64` epoch-seconds value
  (falling back to `0` when the format is unavailable on the
  running tmux version). Pty probes read
  `last_change_atomic.load(Ordering::Acquire)` from `PtyShared`. An
  advance between two consecutive observations signals that
  bytes were written to the target during the `quiet_window`,
  triggering the `Busy` pre-classification (see the
  `Three-State Delivery Classifier` requirement).

The state machine SHALL return the existing
`DeliveryWaitError::{Timeout, Wedged}` variants declared in
`src/transports/contract.rs`. Tmux and Pty SHALL share the state
machine; the per-transport adapter is the only divergence. The
Tmux transport constructs a small `TmuxAsWedgeProbe` adapter that
maps the existing `PaneQuiescenceProbe` into the new generalized
trait, preserving the 16-probe test surface in
`tests/unit/tmux_transport.rs` unchanged. The Pty transport
implements `WedgeProbe` directly in
`src/pty/state.rs::{PtyQuiescenceProbe, WorkerTerminalProbe}`,
populating `WedgeObservation` fields from a shared `PtyShared`
handle.

#### Scenario: Generalized state machine classifies based on probe results

- **WHEN** the shared wedge/prime state machine observes a flush
  group whose probe reports `is_prompt_ready == false` (prompt-
  readiness template does not match the inspected tail)
- **AND** wedge detection is enabled (per-coder config)
- **THEN** the state machine returns `DeliveryWaitError::Wedged { reason }`
  after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
  evaluations, OR when the prime window has elapsed with a
  wedge-class mismatch observed
- **AND** the calling transport maps the error to
  `SendOutcome::Failed` + `reason_code = "pane_wedged"`

#### Scenario: Tmux adapter maps PaneQuiescenceProbe into WedgeProbe::observe

- **WHEN** a Tmux-backed flush group's `TmuxAsWedgeProbe::observe`
  is called
- **THEN** the adapter invokes the underlying `PaneQuiescenceProbe`
  exactly once and packages the result into a `WedgeObservation`
  whose fields (`inspected_tail`, `is_prompt_ready`, `pane_target`,
  `mismatch`, `activity_generation`) reflect the live pane state at
  the moment of the call
- **AND** the Tmux-side wedge/prime semantics match the merged
  `tmux-wedge-detection` and `add-wedge-detection-busy-state`
  proposals unchanged

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft described a four-method trait shape
> (`inspect_tail` / `cursor_idle_at` / `is_settled` /
> `operator_interaction_active`) that was abandoned during
> implementation (per the deviation note recorded in
> `add-pty-transport/tasks.md` §2.2). The shipped two-method
> shape (`observe` / `wait_for_change`) returns a single
> `WedgeObservation` snapshot and is `!Send + !Sync`-safe. The
> `operator_interaction_active` field was retired by
> `remove-operator-interaction-delivery-gate` along with the
> upstream copy-mode gate (issues/relay/52).

