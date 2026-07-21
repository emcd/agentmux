## ADDED Requirements

### Requirement: UI Surface Configuration File

The runtime SHALL support a `ui.toml` operator configuration file for
UI-surface operational defaults, distinct from identity and policy. It SHALL be
read from the same configuration root as `users.toml` at
`<config-root>/ui.toml`.

`ui.toml` SHALL be parsed with kebab-case keys. Supported fields SHALL include:

- `default-bundle` (optional): the bundle the TUI browses by default.

`default-bundle` is a `ui.toml` key; `users.toml` remains the identity and
policy file and does not carry it.

The runtime SHALL treat a missing `ui.toml` as no configured UI-surface
defaults, not an error. A malformed `ui.toml` SHALL fail fast with a structured
bootstrap validation error. Loading `ui.toml` SHALL be read-only and SHALL NOT
scaffold or modify configuration artifacts.

`agentmux check configuration` SHALL validate `ui.toml` through the same
read-only loader, reporting a malformed file with its path and field-level
detail before the relay or TUI would reject it at startup — matching the
pre-flight coverage of `relay.toml` and `users.toml`.

#### Scenario: Resolve default browsing bundle from ui.toml

- **WHEN** the runtime resolves the TUI browsing bundle without `--bundle`
- **AND** `ui.toml` defines `default-bundle`
- **THEN** the runtime resolves the browsing bundle from that `default-bundle`

#### Scenario: Treat missing ui.toml as no UI defaults

- **WHEN** no `ui.toml` exists at the configuration root
- **THEN** the runtime resolves no configured `default-bundle`
- **AND** startup proceeds; the interactive TUI falls back to the first
  available bundle or an empty browsing context

#### Scenario: Fail fast on malformed ui.toml

- **WHEN** `ui.toml` is present but malformed
- **THEN** the runtime fails fast with a structured bootstrap validation error

#### Scenario: Report malformed ui.toml from pre-flight

- **WHEN** an operator runs `agentmux check configuration`
- **AND** `ui.toml` is present but malformed
- **THEN** the command exits non-zero
- **AND** reports the offending file path and field-level detail
