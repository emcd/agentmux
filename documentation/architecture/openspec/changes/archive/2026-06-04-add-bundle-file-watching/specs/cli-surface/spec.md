## ADDED Requirements

### Requirement: Relay Host No-Watch Flag

The `agentmux host relay` subcommand SHALL accept a `--no-watch` flag.
When `--no-watch` is set, the relay SHALL NOT start the bundle file watcher
and SHALL ignore all filesystem changes to the bundles configuration directory
for the lifetime of that process. Absence of `--no-watch` (the default) means
watching is enabled.

#### Scenario: Default — watching enabled

- **WHEN** `agentmux host relay` is executed without `--no-watch`
- **THEN** relay starts the bundle file watcher after initial bundle load completes

#### Scenario: No-watch flag disables watcher

- **WHEN** `agentmux host relay --no-watch` is executed
- **THEN** relay starts without spawning a bundle file watcher
- **AND** filesystem changes to the bundles directory have no effect until relay restart
