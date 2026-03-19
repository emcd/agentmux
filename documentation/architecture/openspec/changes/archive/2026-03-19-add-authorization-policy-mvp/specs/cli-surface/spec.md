## ADDED Requirements
### Requirement: CLI Authorization Adapter Boundary

CLI SHALL remain a validator/adapter surface and SHALL perform no independent
authorization decisioning.
Relay SHALL remain the centralized policy decision point.

#### Scenario: Propagate relay authorization denial unchanged

- **WHEN** relay returns `authorization_forbidden`
- **THEN** CLI surfaces the same code and details schema
- **AND** CLI does not implement command-specific authorization branches

### Requirement: List Command Authorization Semantics

CLI `list` SHALL map to capability `list.read` for authorization outcomes.
If requester identity is valid and policy denies list access, CLI SHALL surface
`authorization_forbidden` and SHALL NOT render an empty successful list.

#### Scenario: Return authorization denial for list request

- **WHEN** operator invokes `agentmux list`
- **AND** policy denies list visibility for resolved requester identity
- **THEN** CLI returns `authorization_forbidden`
- **AND** does not present an empty recipient list as success

## MODIFIED Requirements
### Requirement: Send Target Mode Selection

`agentmux send` SHALL support exactly one target mode per request:

- one or more explicit `--target` values
- `--broadcast`

Send authorization SHALL follow requester policy control scope:

- `all:home`
- `all:all`

#### Scenario: Send to explicit targets

- **WHEN** a caller invokes `agentmux send` with `--target` values
- **THEN** the system routes to exactly those selected recipients

#### Scenario: Send as broadcast

- **WHEN** a caller invokes `agentmux send --broadcast`
- **THEN** the system routes to bundle recipients excluding sender

#### Scenario: Reject conflicting target modes

- **WHEN** a caller provides both explicit `--target` values and `--broadcast`
- **THEN** the system rejects invocation with
  `validation_conflicting_targets`

#### Scenario: Deny cross-bundle send under home-only scope

- **WHEN** caller requests cross-bundle send
- **AND** requester policy `send` scope is `all:home`
- **THEN** CLI surfaces `authorization_forbidden`

### Requirement: Look Command Surface

The system SHALL expose a read-only inspection command:

- `agentmux look <target-session>`

`agentmux look` SHALL support:

- optional `--bundle <name>`
- optional `--lines <n>`

`agentmux look` SHALL return canonical structured JSON output in MVP.
`agentmux look` authorization SHALL use capability label `look.inspect`.
Policy control `look` determines allowed scope (`self`, `all:home`, `all:all`).
Cross-bundle look remains currently unsupported by runtime contract.

#### Scenario: Inspect target session from CLI

- **WHEN** an operator runs `agentmux look <target-session>`
- **THEN** the system requests a read-only snapshot for that target session
- **AND** returns structured JSON payload from relay inspection response

#### Scenario: Use associated bundle when bundle flag is omitted

- **WHEN** an operator runs `agentmux look <target-session>` without `--bundle`
- **THEN** the system uses associated bundle context resolved for the caller

#### Scenario: Reject invalid lines value

- **WHEN** an operator provides `--lines` outside valid range
- **THEN** the system rejects invocation with `validation_invalid_lines`

#### Scenario: Reject cross-bundle look attempt in MVP

- **WHEN** an operator provides `--bundle` outside associated bundle context
- **THEN** the system rejects invocation with
  `validation_cross_bundle_unsupported`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** operator requests look for same-bundle non-self target
- **AND** requester policy `look` scope is `self`
- **THEN** CLI surfaces `authorization_forbidden`
