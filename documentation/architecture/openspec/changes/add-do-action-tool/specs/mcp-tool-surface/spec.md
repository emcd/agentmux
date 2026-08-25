## MODIFIED Requirements
### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools:

- `list`
- `help`
- `look`
- `send`
- `raww`
- `choose`
- `updown`
- `new`
- `change`
- `drop`
- `do`

The relocked pre-stable MCP surface uses `list.principals` with no
compatibility alias for the prior `list.sessions` shape.

The system SHALL NOT expose a temporary `chat` compatibility alias by default.

#### Scenario: Advertise relocked list meta-tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list`
- **AND** includes `help`
- **AND** includes `look`
- **AND** includes `send`
- **AND** includes `raww`
- **AND** includes `choose`
- **AND** does not include `list.sessions`
- **AND** does not include `grant`
- **AND** does not include `chat`

#### Scenario: Advertise admin meta-tools

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `updown`
- **AND** includes `new`
- **AND** includes `change`
- **AND** includes `drop`

#### Scenario: Advertise full tool set including do

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `do`

## ADDED Requirements
### Requirement: MCP Do Tool Modes

The system SHALL expose one MCP tool `do` with mode-based requests:

- `mode=list`
- `mode=show`
- `mode=run`

`mode=run` SHALL require `action`.
`mode=show` SHALL require `action`.
`mode=run` and `mode=show` SHALL reject missing `action` with
`validation_invalid_arguments`.

In alpha scope, `do` requests SHALL NOT include target selector fields
(`target_session`, `target_sessions`).

#### Scenario: List actions via MCP do tool

- **WHEN** a caller invokes `do` with `mode=list`
- **THEN** the system returns configured actions for current session context

#### Scenario: Show action via MCP do tool

- **WHEN** a caller invokes `do` with `mode=show` and `action=compact`
- **THEN** the system returns metadata for action `compact`

#### Scenario: Execute action via MCP do tool

- **WHEN** a caller invokes `do` with `mode=run` and a configured action
- **THEN** the system enqueues action execution and returns structured outcome

#### Scenario: Reject do mode missing action

- **WHEN** a caller invokes `do` with `mode=run` or `mode=show` without
  `action`
- **THEN** the system rejects with `validation_invalid_arguments`

#### Scenario: Reject target selector fields in do request

- **WHEN** a caller provides `target_session` or `target_sessions` in `do`
  request parameters
- **THEN** the system rejects with `validation_invalid_arguments`

### Requirement: MCP Do Execution Safety

`do` execution SHALL enforce action allowlist and self-target-only execution in
alpha scope.

`do` execution targeting the sender's own session SHALL always use effective
async execution semantics.

alpha scope SHALL NOT introduce broader authorization constraints beyond `self-only`;
those are deferred to the existing authorization track.

#### Scenario: Reject unknown do action

- **WHEN** caller runs `do` with unconfigured action id
- **THEN** the system rejects with `validation_unknown_action`

#### Scenario: Reject policy-disallowed do target

- **WHEN** caller attempts to specify target selector fields for `do run`
- **THEN** the system rejects with `validation_invalid_arguments`

#### Scenario: Return accepted outcome for self run

- **WHEN** caller runs `do` for self-target action
- **THEN** response indicates accepted/queued behavior
- **AND** does not require sync completion semantics

### Requirement: MCP Do Run Acceptance Payload

Successful `do` `mode=run` response SHALL include required fields:

- `schema_version`
- `bundle_name`
- `requester_session`
- `action`
- `status` (`accepted`)
- `outcome` (`queued`)
- `message_id`

#### Scenario: Return canonical acceptance payload for do run

- **WHEN** caller invokes `do` with `mode=run` and configured `action`
- **THEN** response includes all required acceptance payload fields
