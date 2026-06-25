## MODIFIED Requirements

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals from the background reader to
maintain worker readiness state for scheduling. The readiness state ACP
maintains SHALL be the shared transport-neutral `WorkerReadinessState` recorded
in the relay's transport-agnostic worker-readiness registry (see the
transport-abstraction "Worker Readiness Interface" requirement); ACP is one
populator of that registry, not the owner of a private readiness type. The
transition triggers below remain ACP-specific.

State model:

- `available`: worker healthy and ready for next prompt
- `busy`: prompt accepted and turn in progress
- `unavailable`: worker transport/process failure requiring restart

Transition contract:

- successful prompt write to ACP stdin => `busy`
- background reader observes terminal `stopReason` in prompt response => `available`
- stdin write failure OR reader thread exit (any cause) => `unavailable`

The `busy` transition now occurs on write-success, not on first `session/update`
observation. Reader thread exit is an additional `unavailable` trigger,
mirroring the write-failure path.

Sender-surface contract:

- these transitions SHALL NOT require additional sender-facing `send` outputs
- send success semantics remain phase-1 delivery acknowledgment only

#### Scenario: Mark worker busy on prompt write success

- **WHEN** relay successfully writes a prompt to ACP stdin
- **THEN** relay marks worker state as `busy`

#### Scenario: Mark worker available on terminal stopReason

- **WHEN** ACP background reader receives JSON-RPC response to the prompt
  request-id with a terminal `stopReason`
- **THEN** relay marks worker state as `available`
- **AND** subsequent sends MAY be admitted for that target

#### Scenario: Mark worker unavailable on reader thread exit

- **WHEN** the ACP background reader thread exits (EOF, I/O error, or panic)
- **THEN** relay marks worker state as `unavailable`
- **AND** pending requests are drained with an error

#### Scenario: ACP populates the shared worker-readiness registry

- **WHEN** ACP records any readiness transition
- **THEN** it writes a `WorkerReadinessState` value into the transport-agnostic
  worker-readiness registry via `set_worker_readiness`
- **AND** observers of `subscribe_worker_readiness` / `read_worker_readiness` see
  the transition without any ACP-specific observer name
