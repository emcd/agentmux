## MODIFIED Requirements

### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command with these
subcommands:

- `host relay <bundle-id>`
- `host mcp`
- `list`
- `send`
- `tui`

The system SHALL retain `agentmux-relay` and `agentmux-mcp` as compatibility
entrypoints.

#### Scenario: Launch TUI from unified command

- **WHEN** an operator runs `agentmux tui`
- **THEN** the system starts TUI workflow

### Requirement: Bare Agentmux TUI Dispatch

When invoked without a subcommand, `agentmux` SHALL dispatch based on terminal
context:

- interactive TTY invocation starts TUI workflow,
- non-TTY invocation fails fast by printing help and exiting non-zero.

#### Scenario: Launch bare agentmux on TTY

- **WHEN** an operator runs `agentmux` without a subcommand
- **AND** the process is attached to an interactive TTY
- **THEN** the system starts TUI workflow as if `agentmux tui` was invoked

#### Scenario: Launch bare agentmux without TTY

- **WHEN** an operator runs `agentmux` without a subcommand
- **AND** the process is not attached to an interactive TTY
- **THEN** the system prints CLI help output
- **AND** exits with a non-zero status code

## ADDED Requirements

### Requirement: TUI Sender Override Precedence Hook

`agentmux tui` SHALL support optional `--sender <session-id>`.

When `--sender` is provided, it SHALL have higher precedence than
configuration-based or association-based sender resolution.

#### Scenario: Launch TUI with explicit sender override

- **WHEN** an operator runs `agentmux tui --sender relay`
- **AND** a sender is also configured via override or normal `tui.toml`
- **THEN** TUI startup sender identity resolves to `relay`

#### Scenario: Launch TUI without explicit sender override

- **WHEN** an operator runs `agentmux tui` without `--sender`
- **THEN** TUI startup sender identity resolves via configured precedence
