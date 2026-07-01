## RENAMED Requirements

- FROM: `### Requirement: Relay Host No-Watch Flag`
- TO: `### Requirement: Relay Host Relay Configuration Controls`

## MODIFIED Requirements

### Requirement: Relay Host Relay Configuration Controls

The `agentmux host relay` subcommand SHALL resolve relay-wide runtime controls
from CLI overrides, environment overrides, `<config-root>/relay.toml`, and
documented defaults, in that order. The `--no-watch` and
`--require-credentials` flags SHALL remain supported as CLI overrides for the
configuration file rather than as the durable source of truth.

When runtime configuration resolves `watch-bundles = false`, the relay SHALL NOT
start the bundle file watcher and SHALL ignore all filesystem changes to the
bundles configuration directory for the lifetime of that process. When the
setting resolves absent or `true`, watching is enabled.

When runtime configuration resolves `require-session-credentials = true`, the
relay SHALL enforce recognized session credentials on Hello. When the key is
absent or `false`, socket-trusted session connections remain allowed.

#### Scenario: Default configuration enables watching

- **WHEN** `agentmux host relay` is executed
- **AND** no CLI, environment, or `relay.toml` watch setting is supplied
- **THEN** relay starts the bundle file watcher after initial bundle load
  completes

#### Scenario: Relay configuration disables watcher

- **WHEN** `agentmux host relay` is executed
- **AND** `relay.toml` sets `watch-bundles = false`
- **THEN** relay starts without spawning a bundle file watcher
- **AND** filesystem changes to the bundles directory have no effect until relay
  restart

#### Scenario: Watch CLI override disables watcher

- **WHEN** an operator runs `agentmux host relay --no-watch`
- **THEN** relay starts without spawning a bundle file watcher

#### Scenario: Credential CLI override enforces credentials

- **WHEN** an operator runs `agentmux host relay --require-credentials`
- **THEN** relay enforces recognized session credentials on Hello
