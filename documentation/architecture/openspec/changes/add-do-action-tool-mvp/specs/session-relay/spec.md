## ADDED Requirements
### Requirement: Configured Do Action Registry

The system SHALL support configured do-action entries for relay-dispatched
automation prompts.

Action entries SHALL be defined in `coders.toml` for each coder so prompts can
vary by coder/session context.

Each action definition SHALL include:

- `id` (unique action key)
- `prompt` (template/prompt text to inject)
- optional `description`
- optional `self-only` (default `true`)

Action definitions SHALL be resolved from active runtime configuration for the
sender/session context.

#### Scenario: Load configured action registry

- **WHEN** runtime configuration includes action definitions
- **THEN** relay resolves those actions for eligible sessions

#### Scenario: Reject duplicate action ids

- **WHEN** configuration contains duplicate action ids in one action set
- **THEN** system rejects configuration with validation error

### Requirement: Relay Do Operation

Relay SHALL expose a `do` operation with modes:

- `list`
- `show`
- `run`

`list` returns available action ids and optional descriptions.
`show` returns metadata for one action.
`run` dispatches configured prompt injection for one action.

#### Scenario: Return available actions for list mode

- **WHEN** relay receives `do` request with `mode=list`
- **THEN** relay returns action catalog for sender/session context

#### Scenario: Return action metadata for show mode

- **WHEN** relay receives `do` request with `mode=show` for configured
  action id
- **THEN** relay returns metadata for that action

#### Scenario: Reject unknown action on run mode

- **WHEN** relay receives `do` run request for unknown action id
- **THEN** relay returns `validation_unknown_action`

### Requirement: Relay Do Safety and Execution Semantics

Relay do execution SHALL enforce:

- action allowlist from configuration
- action target policy (`self-only`)
- effective async behavior for self-target actions

MVP SHALL NOT introduce broader authorization constraints beyond `self-only`;
those are deferred to the existing authorization track.

Relay SHALL emit action lifecycle inscriptions for observability.

#### Scenario: Enforce self-only policy

- **WHEN** action has `self-only=true`
- **AND** run request targets non-self session
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Force async behavior for self run

- **WHEN** run request targets sender session
- **THEN** relay treats dispatch as accepted/queued
- **AND** does not block waiting for sync completion semantics

#### Scenario: Emit do lifecycle inscriptions

- **WHEN** relay processes do run request
- **THEN** relay emits inscriptions for request and downstream delivery
  lifecycle events
