## MODIFIED Requirements

### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command with these
subcommands:

- `host relay`
- `host mcp`
- `up`
- `down`
- `list`
- `send`
- `look`
- `check configuration`

The system SHALL retain `agentmux-relay` and `agentmux-mcp` as compatibility
entrypoints.

#### Scenario: Expose bundle lifecycle commands in topology

- **WHEN** an operator views `agentmux --help`
- **THEN** the CLI includes `up` and `down` subcommands

#### Scenario: Host relay from unified command

- **WHEN** an operator runs `agentmux host relay`
- **THEN** the system starts relay hosting flow

#### Scenario: Host MCP from unified command

- **WHEN** an operator runs `agentmux host mcp`
- **THEN** the system starts MCP hosting flow with configured association
  resolution

#### Scenario: Pre-flight configuration from unified command

- **WHEN** an operator runs `agentmux check configuration`
- **THEN** the system validates bundle configuration without starting the relay

#### Scenario: Preserve legacy binary entrypoints

- **WHEN** an operator runs `agentmux-relay` or `agentmux-mcp`
- **THEN** the command remains supported
- **AND** behavior remains equivalent to the unified host command paths

## ADDED Requirements

### Requirement: Configuration Pre-flight Command Surface

The system SHALL provide an `agentmux check configuration [<bundle-id>]`
subcommand that validates bundle configuration through the same loading path the
relay uses at startup, without starting the relay or mutating configuration.

The command SHALL accept an optional positional `<bundle-id>`: when present it
validates that single bundle; when omitted it validates every discoverable
bundle under the configuration root.

The command SHALL inherit the global runtime flags (`--config-directory`,
`--state-directory`, `--inscriptions-directory`/`--logs-directory`,
`--repository-root`).

Validation SHALL cover bundle and coders schema and authorization-policy
resolution (`policies.toml`, `relay.toml`, and `users.toml` policy mappings),
matching what the relay rejects at startup.

The command SHALL be read-only: it MUST NOT scaffold or modify configuration
artifacts.

On success the command SHALL exit zero. On the first invalid bundle it SHALL
exit non-zero and report the offending file path and field-level detail; it does
not partially load or degrade gracefully.

#### Scenario: Validate a single named bundle

- **WHEN** an operator runs `agentmux check configuration <bundle-id>` against a
  valid configuration
- **THEN** the command exits zero
- **AND** reports the bundle as validated

#### Scenario: Validate all bundles when no id is given

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument
- **THEN** the command validates every discoverable bundle
- **AND** exits zero when all are valid

#### Scenario: Report an unknown configuration field

- **WHEN** a bundle file contains an unknown field (for example a misspelled
  session key)
- **THEN** the command exits non-zero
- **AND** reports the offending file path and the offending field

#### Scenario: Reject an unknown check subcommand

- **WHEN** an operator runs `agentmux check <other>` where `<other>` is not
  `configuration`
- **THEN** the command rejects the invocation with a structured argument
  validation error

#### Scenario: Report when no bundles are discoverable

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument and no bundle files exist
- **THEN** the command exits non-zero
- **AND** reports that no bundle configurations were found
