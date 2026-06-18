## MODIFIED Requirements

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

## ADDED Requirements

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
