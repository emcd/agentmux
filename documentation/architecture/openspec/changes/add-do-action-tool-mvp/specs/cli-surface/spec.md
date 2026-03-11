## MODIFIED Requirements
### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command with these
subcommands:

- `host relay <bundle-id>`
- `host mcp`
- `list`
- `look <target-session>`
- `send`
- `do`

The system SHALL retain `agentmux-relay` and `agentmux-mcp` as compatibility
entrypoints.

#### Scenario: Expose do command in unified topology

- **WHEN** an operator runs `agentmux --help`
- **THEN** the command list includes `do`

## ADDED Requirements
### Requirement: Do Action Command Surface

The system SHALL expose a configured action dispatcher through:

- `agentmux do` (list mode)
- `agentmux do --show <action>` (show mode)
- `agentmux do <action>` (execute mode)

List mode SHALL return available configured actions for the resolved session
context.

Execute mode SHALL run only configured actions and SHALL reject unknown action
ids. Execute mode SHALL target the caller's own session in MVP and SHALL NOT
accept target selector arguments.

#### Scenario: List available actions in list mode

- **WHEN** an operator runs `agentmux do`
- **THEN** the system returns configured action ids and short descriptions

#### Scenario: Execute configured action

- **WHEN** an operator runs `agentmux do compact`
- **THEN** the system dispatches configured action `compact`
- **AND** returns structured execution acceptance metadata

#### Scenario: Reject unknown action id

- **WHEN** an operator runs `agentmux do unknown-action`
- **THEN** the system rejects invocation with `validation_unknown_action`

### Requirement: Do Action Show Query

The CLI SHALL support action metadata query mode for one action.

MVP SHALL express this as `agentmux do --show <action>`.

Show output SHALL include:

- action id
- description (when configured)
- parameter model for execution payload (MVP may be empty/none)
- self-target policy (`self-only`)

#### Scenario: Show configured action metadata

- **WHEN** an operator queries one action in show mode
- **THEN** the system returns action metadata and execution policy

### Requirement: Do Action Safety Semantics

Action execution SHALL enforce:

- configured action allowlist
- self-target-only execution in MVP (no target selector arguments)
- effective async execution for self-target runs

MVP SHALL NOT introduce broader authorization constraints beyond the `self-only`
policy; those are deferred to the existing authorization track.

#### Scenario: Force async semantics for self action run

- **WHEN** an operator executes `agentmux do <action>` against own session
- **THEN** execution uses effective async behavior
- **AND** does not block waiting for prompt quiescence completion outcome

#### Scenario: Reject target selector argument in MVP

- **WHEN** an operator provides target selector arguments to `agentmux do`
- **THEN** the system rejects with `validation_invalid_arguments`

### Requirement: Do Run Acceptance Payload

Successful `agentmux do <action>` execution SHALL return a structured
acceptance payload with required fields:

- `schema_version`
- `bundle_name`
- `requester_session`
- `action`
- `status` (`accepted`)
- `outcome` (`queued`)
- `message_id`

#### Scenario: Return canonical acceptance payload for run mode

- **WHEN** an operator runs `agentmux do compact`
- **THEN** the command returns all required acceptance payload fields
