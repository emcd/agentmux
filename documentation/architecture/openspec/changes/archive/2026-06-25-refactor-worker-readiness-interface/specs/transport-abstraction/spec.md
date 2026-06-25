## ADDED Requirements

### Requirement: Worker Readiness Interface

The relay SHALL expose worker readiness through a transport-agnostic interface,
not an ACP-specific one. Any worker-driven transport (ACP today, Pty next) that
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
stopReason mechanics) SHALL remain in the owning transport module
(`src/acp` today, `src/pty` as the named future example), which drives the
shared interface rather than defining its own readiness type. Tmux does not
drive a multi-state worker readiness lifecycle and is therefore not a populator
of this interface.

#### Scenario: ACP worker populates the shared readiness interface

- **WHEN** the ACP worker transitions readiness (e.g. to `busy` on prompt-write
  success)
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
