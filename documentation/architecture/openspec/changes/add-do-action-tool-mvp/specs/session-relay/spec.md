## ADDED Requirements
### Requirement: Configured Do Action Registry

The system SHALL support configured do-action entries for relay-dispatched
automation prompts.

Action entries SHALL be defined in `coders.toml` at canonical path
`[[coders.do-actions]]` for each coder so prompts can vary by
coder/session context.

Each action definition SHALL include:

- `id` (unique action key)
- `prompt` (template/prompt text to inject)
- optional `description`
- optional `self-only` (default `true`)

In MVP, `self-only` is a forward-compat policy field; non-self targeting is not
supported yet, so do-run behavior remains self-target-only regardless of this
field value.

Action definitions SHALL be resolved from active runtime configuration for the
sender/session context.

#### Scenario: Load configured action registry from canonical coder path

- **WHEN** `coders.toml` defines `[[coders]]` entries with nested
  `[[coders.do-actions]]` tables
- **THEN** relay resolves do-action entries from that canonical path

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

Run mode request contract SHALL include:

- required `mode=run`
- required `action`
- no target selector fields in MVP

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

#### Scenario: Reject target selector fields for do run

- **WHEN** relay receives do run request with `target_session` or
  `target_sessions`
- **THEN** relay returns `validation_invalid_arguments`

### Requirement: Relay Do Safety and Execution Semantics

Relay do execution SHALL enforce:

- action allowlist from configuration
- self-target-only execution in MVP
- effective async behavior for self-target actions

MVP SHALL NOT introduce broader authorization constraints beyond `self-only`;
those are deferred to the existing authorization track.

Relay SHALL emit action lifecycle inscriptions for observability.

#### Scenario: Force async behavior for self run

- **WHEN** relay receives a valid do run request (self-target by MVP contract)
- **THEN** relay treats dispatch as accepted/queued
- **AND** does not block waiting for sync completion semantics

#### Scenario: Emit do lifecycle inscriptions

- **WHEN** relay processes do run request
- **THEN** relay emits inscriptions for request and downstream delivery
  lifecycle events

### Requirement: Relay Do Run Acceptance Payload

Successful `do` `run` response SHALL include required fields:

- `schema_version`
- `bundle_name`
- `requester_session`
- `action`
- `status` (`accepted`)
- `outcome` (`queued`)
- `message_id`

#### Scenario: Return canonical acceptance payload for do run

- **WHEN** relay accepts `do` run request for configured action
- **THEN** relay response includes all required acceptance payload fields
