## MODIFIED Requirements
### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for the relay MVP:

- `list`
- `look`
- `send`
- `do`

The system SHALL NOT expose a temporary `chat` compatibility alias by default.

#### Scenario: Advertise full tool set including do

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `do` in addition to existing tools

## ADDED Requirements
### Requirement: MCP Do Tool Modes

The system SHALL expose one MCP tool `do` with mode-based requests:

- `mode=list`
- `mode=show`
- `mode=run`

`mode=run` SHALL require `action`.
`mode=show` SHALL require `action`.

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

### Requirement: MCP Do Execution Safety

`do` execution SHALL enforce action allowlist and `self-only` policy.

`do` execution targeting the sender's own session SHALL always use effective
async execution semantics.

MVP SHALL NOT introduce broader authorization constraints beyond `self-only`;
those are deferred to the existing authorization track.

#### Scenario: Reject unknown do action

- **WHEN** caller runs `do` with unconfigured action id
- **THEN** the system rejects with `validation_unknown_action`

#### Scenario: Reject policy-disallowed do target

- **WHEN** action policy disallows requested target session
- **THEN** the system rejects with `authorization_forbidden`

#### Scenario: Return accepted outcome for self run

- **WHEN** caller runs `do` for self-target action
- **THEN** response indicates accepted/queued behavior
- **AND** does not require sync completion semantics
