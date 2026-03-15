## MODIFIED Requirements

### Requirement: Initial TUI MVP Workflow Coverage

The system SHALL define an initial TUI MVP that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery-events inspection and pending indicator.

The MVP SHALL reuse existing relay delivery and inspection semantics.
The MVP SHALL consume inbound message and delivery-outcome updates from relay
stream transport contracts defined in
`add-relay-stream-hello-transport-mvp`.

#### Scenario: Cover core operator loop with structured update flow

- **WHEN** an operator uses the TUI for routine coordination
- **THEN** the TUI supports recipient selection, send, look, and update
  workflows
- **AND** inbound message and delivery outcome updates are represented using
  canonical relay stream event payloads

## ADDED Requirements

### Requirement: TUI Sender Identity Precedence

`agentmux tui` sender identity SHALL resolve with deterministic precedence:

1. CLI `--sender` when provided
2. local testing override `tui.toml` sender when active
3. normal `<config-root>/tui.toml` sender
4. runtime association-derived sender identity
5. fail-fast validation error when unresolved

TUI runtime SHALL use the resolved sender identity consistently for relay-backed
operations in that TUI process.

#### Scenario: Prefer CLI sender over configured sender files

- **WHEN** operator starts TUI with `--sender`
- **AND** sender is also configured via override or normal `tui.toml`
- **THEN** TUI resolves sender from `--sender`

#### Scenario: Fail fast when sender cannot be resolved

- **WHEN** CLI sender is absent
- **AND** configured sender files are absent or inapplicable
- **AND** runtime association cannot resolve a valid sender
- **THEN** TUI startup fails with stable validation error code

### Requirement: TUI Delivery State Mapping

TUI state and history surfaces SHALL use this outcome vocabulary:

- `accepted`
- `success`
- `timeout`
- `failed`

Mapping rules SHALL be:

- async send acceptance maps to `accepted`
- terminal delivered outcome maps to `success`
- terminal timeout outcome maps to `timeout`
- terminal non-timeout failure maps to `failed`
- terminal relay state `dropped_on_shutdown` maps to `failed` with
  `reason_code=dropped_on_shutdown`

`accepted` is process-local state derived from send acknowledgement and SHALL
NOT require replay after reconnect.

#### Scenario: Represent async send acceptance as accepted

- **WHEN** relay accepts an async send request for one or more targets
- **THEN** TUI records initial delivery state `accepted` for those targets

#### Scenario: Transition accepted state to terminal outcome

- **WHEN** TUI receives terminal delivery update for an accepted target
- **THEN** TUI updates state to exactly one of `success`, `timeout`, or
  `failed`

#### Scenario: Treat accepted state as local on reconnect

- **WHEN** TUI reconnects stream handling after process restart
- **THEN** TUI does not require replay of `accepted` lifecycle events
- **AND** applies terminal outcomes from relay stream events

### Requirement: TUI Transport Failure Semantics

TUI SHALL surface transport/connectivity failures explicitly and SHALL NOT
silently degrade into synthetic success states.

Same-bundle MVP lock SHALL remain enforced for transport and history updates.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Reject cross-bundle transport scope in MVP

- **WHEN** TUI transport scope attempts bundle outside associated context
- **THEN** request is rejected with `validation_cross_bundle_unsupported`
