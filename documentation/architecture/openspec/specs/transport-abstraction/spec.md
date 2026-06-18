# transport-abstraction Specification

## Purpose
TBD - created by archiving change decouple-transport-layer. Update Purpose after archive.
## Requirements
### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL dispatch all agent delivery operations
through a synchronous `Transport` trait defined in `src/transports/contract.rs`.
Each transport type (ACP, Tmux) SHALL implement the trait in its own module.
The relay SHALL dispatch via a `TransportImpl` enum that delegates to the
appropriate implementation without dynamic allocation.

The trait methods SHALL be synchronous. The relay delivery worker is responsible
for wrapping transport calls in `spawn_blocking` where needed; each transport
implementation SHALL NOT impose async boundaries on the relay worker.

#### Scenario: ACP delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to an ACP target
- **THEN** it calls `TransportImpl::Acp(t).deliver(envelopes, context)`
- **AND** the `AcpTransport` implementation handles all ACP-specific protocol
  details within the sync call

#### Scenario: Tmux delivery via TransportImpl

- **WHEN** the relay delivery worker delivers to a Tmux target
- **THEN** it calls `TransportImpl::Tmux(t).deliver(envelopes, context)`
- **AND** the `TmuxTransport` implementation handles pane injection within the
  sync call (quiescence is gated upstream by `prepare_delivery`; see the
  Pre-Delivery Readiness Barrier requirement)

### Requirement: Transport Module Boundaries

ACP-specific delivery code SHALL reside in `src/acp/`. Tmux-specific delivery
code SHALL reside in `src/tmux/`. After completion of Slice 4, the relay
delivery subsystem SHALL NOT contain transport-specific logic; all transport
dispatch SHALL go through `TransportImpl`.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations or session lifecycle primitives are present

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

`deliver()` SHALL block until each envelope reaches a terminal state and SHALL
return one terminal outcome per envelope; the relay worker performs sender
fan-out from the return value, not from a transport-issued completion callback or
event. `deliver()` SHALL observe relay shutdown and return a terminal/dropped
outcome promptly rather than parking the blocking thread indefinitely. This does
not block the relay request path: the send RPC returns `Queued` at enqueue, and
`deliver()` blocks only the per-target worker's `spawn_blocking` thread.

#### Scenario: deliver returns the terminal outcome

- **WHEN** the relay worker delivers an ACP batch
- **THEN** `deliver()` returns only once the turn reaches a terminal state
- **AND** the returned `DeliveryResult` carries the per-envelope terminal outcome

#### Scenario: deliver yields on shutdown

- **WHEN** relay shutdown is requested while `deliver()` is awaiting completion
- **THEN** `deliver()` returns a terminal/dropped outcome promptly

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

### Requirement: Pre-Delivery Readiness Barrier

Before committing a batch, the relay delivery worker SHALL call
`Transport::prepare_delivery(context)` to gate delivery on the target being ready
to receive it. A transport whose readiness is observable (Tmux pane quiescence;
Pty fd idle, when it lands) SHALL perform the wait and return the resolved
target; a transport with no pre-delivery wait (ACP today) SHALL return ready
immediately. The barrier SHALL run as a distinct relay-side step so the worker
can drain task arrivals into the batch during the wait (coalesce-during-wait); it
SHALL NOT be folded into `deliver()`. On timeout, shutdown, or target
unavailability the barrier SHALL return an error that the worker fans across the
coalesced batch. The resolved target SHALL ride in `DeliveryContext`'s
`pre_resolved_target`; a transport whose handle is not a string MAY re-resolve in
its own `deliver()`.

#### Scenario: Tmux waits for pane quiescence before paste

- **WHEN** the relay delivers an envelope batch to a Tmux target
- **THEN** `prepare_delivery()` waits for the pane to fall quiescent and returns
  the resolved pane
- **AND** tasks arriving during the wait are coalesced into the batch before paste

#### Scenario: ACP barrier returns ready immediately

- **WHEN** the relay delivers to an ACP target
- **THEN** `prepare_delivery()` returns ready without a wait

### Requirement: Relay-Combined Batch Dispatch

The relay delivery worker SHALL pre-combine a coalesced turn before dispatch and
fan one terminal outcome out to every coalesced task; a transport SHALL paste
every rendered envelope it receives, in order, within a single `deliver()` call.
A transport that accepts at most one prompt batch per dispatch SHALL declare so
via `SessionType::can_take_batches()` returning `false`; the relay packs
envelopes to the token budget and peels the tail back to the worker carry buffer
to honor that limit, with no transport-specific knowledge of the budget.

#### Scenario: Tmux accepts the full coalesced batch

- **WHEN** the relay delivers a coalesced batch to a Tmux target
- **THEN** `can_take_batches()` is `true`
- **AND** the transport pastes every rendered prompt batch in one `deliver()` call

#### Scenario: ACP accepts one prompt batch per turn

- **WHEN** the relay delivers a coalesced batch to an ACP target
- **THEN** `can_take_batches()` is `false`
- **AND** the relay peels the rendered envelopes to a single prompt batch and
  re-queues the remainder to the worker carry buffer for the next turn

