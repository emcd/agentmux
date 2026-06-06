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

### Requirement: Transport Inbound Event Channel

Transports with push-based inbound events SHALL expose an
`mpsc::Receiver<TransportEvent>` via `inbound()` (ACP: replay entries and
permission requests). Transports without push-based inbound events (Tmux) SHALL
return `None` from `inbound()`.

Each `startup()` call invalidates any previous inbound channel. The relay
delivery worker SHALL re-call `inbound()` and replace its stored receiver
after every `startup()` call, at both initial bootstrap and respawn sites.
The worker SHALL treat a `None` result from polling the inbound receiver as an
expected respawn signal and re-subscribe, not as a delivery error.

#### Scenario: ACP inbound channel on startup

- **WHEN** the relay calls `startup()` on an `AcpTransport`
- **THEN** the transport creates a fresh mpsc channel internally
- **AND** a subsequent `inbound()` call returns `Some(receiver)` connected
  to the new channel

#### Scenario: Re-subscribe after ACP respawn

- **WHEN** the relay calls `startup()` again on an existing `AcpTransport`
  (respawn path)
- **THEN** the old inbound channel is invalidated
- **AND** the relay re-calls `inbound()` and replaces its stored receiver
  at both `bootstrap_acp_runtime_on_worker_start` and
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
