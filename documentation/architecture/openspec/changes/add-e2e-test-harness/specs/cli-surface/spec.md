## ADDED Requirements

### Requirement: Test Command Surface

The system SHALL expose a `test` subcommand grouping test-harness operations:

- `agentmux test harness run --script <path> [--target-bundle <name>]`
  stands up the `qa-harness` Coordinator simulator relay peer in a
  test bundle, runs the script against the target bundle, and exits
  non-zero on assertion failure.
- `agentmux test bundle up <name>` and
  `agentmux test bundle down <name>` host and tear down a test
  bundle explicitly, only when the bundle has `test-isolated=true`
  in its bundle configuration.

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
