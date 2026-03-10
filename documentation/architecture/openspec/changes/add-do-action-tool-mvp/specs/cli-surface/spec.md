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

- `agentmux do` (survey mode)
- `agentmux do <action>` (execute mode)

Survey mode SHALL return available configured actions for the resolved session
context.

Execute mode SHALL run only configured actions and SHALL reject unknown action
ids.

#### Scenario: List available actions in survey mode

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

MVP MAY express this as `agentmux do --show <action>` (or an equivalent
single-action query shape) as long as it is machine-parsable and documented.

Describe output SHALL include:

- action id
- description (when configured)
- parameter model for execution payload (MVP may be empty/none)
- self-target policy (`self-only`)

#### Scenario: Describe configured action metadata

- **WHEN** an operator queries one action in show mode
- **THEN** the system returns action metadata and execution policy

### Requirement: Do Action Safety Semantics

Action execution SHALL enforce:

- configured action allowlist
- target policy checks (`self-only`)
- effective async execution for self-target runs

MVP SHALL NOT introduce broader authorization constraints beyond the `self-only`
policy; those are deferred to the existing authorization track.

#### Scenario: Force async semantics for self action run

- **WHEN** an operator executes `agentmux do <action>` against own session
- **THEN** execution uses effective async behavior
- **AND** does not block waiting for prompt quiescence completion outcome

#### Scenario: Reject disallowed non-self execution

- **WHEN** action policy is `self-only=true`
- **AND** caller targets a different session
- **THEN** the system rejects with `authorization_forbidden`
