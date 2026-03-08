## ADDED Requirements

### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command with these
subcommands:

- `host relay <bundle-id>`
- `host mcp`
- `list`
- `send`

The system SHALL retain `agentmux-relay` and `agentmux-mcp` as compatibility
entrypoints.

#### Scenario: Host relay from unified command

- **WHEN** an operator runs `agentmux host relay <bundle-id>`
- **THEN** the system starts relay hosting flow for the selected bundle

#### Scenario: Host MCP from unified command

- **WHEN** an operator runs `agentmux host mcp`
- **THEN** the system starts MCP hosting flow with configured association
  resolution

#### Scenario: Preserve legacy binary entrypoints

- **WHEN** an operator runs `agentmux-relay` or `agentmux-mcp`
- **THEN** the command remains supported
- **AND** behavior remains equivalent to the unified host command paths

### Requirement: Relay Host Bundle Selection

`agentmux host relay` SHALL require bundle identity as a positional argument:
`<bundle-id>`.

#### Scenario: Reject missing relay bundle identifier

- **WHEN** an operator runs `agentmux host relay` without `<bundle-id>`
- **THEN** the system rejects invocation with a structured argument validation
  error

### Requirement: Send Target Mode Selection

`agentmux send` SHALL support exactly one target mode per request:

- one or more explicit `--target` values
- `--broadcast`

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

### Requirement: Send Message Input Resolution

`agentmux send` SHALL resolve message body from exactly one source:

- `--message`
- piped stdin

If `--message` is omitted and piped stdin is available, stdin content SHALL be
used as the message body.

If both sources are present, the system SHALL reject invocation with
`validation_conflicting_message_input`.

If neither source is present, the system SHALL reject invocation with
`validation_missing_message_input`.

The MVP system SHALL NOT enter interactive line-capture mode when stdin is a
TTY and `--message` is omitted.

#### Scenario: Read message body from option flag

- **WHEN** a caller invokes `agentmux send --message "Hello"`
- **THEN** the system uses the provided flag value as message body

#### Scenario: Read message body from piped stdin

- **WHEN** a caller invokes `agentmux send` without `--message`
- **AND** stdin is piped with non-empty content
- **THEN** the system uses stdin content as message body

#### Scenario: Reject conflicting message sources

- **WHEN** a caller provides `--message`
- **AND** stdin is piped with message content
- **THEN** the system rejects invocation with
  `validation_conflicting_message_input`

#### Scenario: Reject missing message source in non-piped mode

- **WHEN** a caller invokes `agentmux send` without `--message`
- **AND** stdin is a TTY
- **THEN** the system rejects invocation with
  `validation_missing_message_input`
