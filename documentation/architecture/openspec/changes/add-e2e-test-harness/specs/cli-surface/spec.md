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
- `test`

The `test` subcommand groups test-harness operations:

- `agentmux test harness run --script <path> [--target-bundle <name>]`
  stands up the `qa-harness` Coordinator simulator relay peer in a
  test bundle, runs the script against the target bundle, and exits
  non-zero on assertion failure.
- `agentmux test bundle up <name>` and
  `agentmux test bundle down <name>` host and tear down a test
  bundle explicitly, only when the bundle has `test-isolated=true`
  in its bundle configuration.

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

#### Scenario: Preserve legacy binary entrypoints

- **WHEN** an operator runs `agentmux-relay` or `agentmux-mcp`
- **THEN** the command remains supported
- **AND** behavior remains equivalent to the unified host command paths

#### Scenario: Expose test subcommand in topology

- **WHEN** an operator views `agentmux --help`
- **THEN** the CLI includes the `test` subcommand
- **AND** `agentmux test --help` lists `harness` and `bundle` subcommands

#### Scenario: Test harness run executes a script

- **WHEN** an operator runs
  `agentmux test harness run --script /path/to/script.toml`
- **THEN** the system locates the script file
- **AND** stands up the test bundle via `agentmux test bundle up`
- **AND** starts the harness relay peer
- **AND** runs the script's operations in order
- **AND** exits 0 if all operations complete and assertions pass
- **AND** exits non-zero if any operation times out or any assertion fails

#### Scenario: Test bundle up rejects non-isolated bundle

- **WHEN** an operator runs
  `agentmux test bundle up production-bundle`
- **AND** `production-bundle` does NOT have `test-isolated=true` in its
  bundle configuration
- **THEN** the command fails with a clear error message
- **AND** does not modify relay hosting state
