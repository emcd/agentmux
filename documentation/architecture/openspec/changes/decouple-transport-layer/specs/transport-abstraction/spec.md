## ADDED Requirements

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
- **AND** the `TmuxTransport` implementation handles pane injection and
  quiescence polling within the sync call

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

A transport with observable output SHALL publish an `OutputView` handle via
`give_output()`; transports with no observable output SHALL return `None`. The
relay SHALL read the handle from the `look` request path, which runs
concurrently with the worker that owns the transport. The relay SHALL re-fetch
the handle after every `startup()` call, at both initial bootstrap and respawn
sites. The handle SHALL own the bounded prime-wait (waiting up to
`LookMode::prime_timeout` for a still-initializing target) and SHALL return ACP
freshness metadata so the relay remains transport-generic. A `look` racing a
respawn SHALL return stale/unavailable metadata or a clean `TransportError`,
never a panic or a read of the wrong target's state.

#### Scenario: ACP look reads the published handle

- **WHEN** a `look` request targets an ACP session
- **THEN** the relay reads the `OutputView` handle published by `give_output()`
- **AND** the handle returns the replay snapshot plus freshness metadata without
  borrowing the worker-owned transport

#### Scenario: Re-fetch handle after ACP respawn

- **WHEN** the relay calls `startup()` again on an existing `AcpTransport`
  (respawn path)
- **THEN** the relay re-fetches the handle via `give_output()` and replaces its
  stored handle at both `bootstrap_acp_runtime_on_worker_start` and
  `drive_acp_worker_respawn` callsites

### Requirement: Transport Capacity Declaration

Each transport SHALL declare per-call delivery capacity via `accept_capacity()`.
The relay delivery worker SHALL split batches to fit the declared capacity
without transport-specific knowledge of the limit.

#### Scenario: Tmux capacity enforces single delivery

- **WHEN** the relay attempts to deliver a batch to a Tmux target
- **THEN** `accept_capacity()` returns 1
- **AND** the worker delivers exactly one envelope per `deliver()` call

#### Scenario: ACP accepts larger batches

- **WHEN** the relay attempts to deliver a batch to an ACP target
- **THEN** `accept_capacity()` returns up to the full batch size
- **AND** the worker delivers all accepted envelopes in a single `deliver()`
  call
