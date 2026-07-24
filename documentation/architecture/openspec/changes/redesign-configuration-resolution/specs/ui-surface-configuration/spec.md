## MODIFIED Requirements

### Requirement: UI Surface Configuration File

The runtime SHALL support a `ui.toml` operator configuration file for
UI-surface operational defaults, distinct from identity and policy. It SHALL be
resolved at relative path `ui.toml` through the shared effective-file lookup, so
an overlay-provided `ui.toml` shadows the base file. This replaces the previous
configuration-root-only behavior, aligning `ui.toml` with `users.toml`.

`ui.toml` SHALL be parsed with kebab-case keys. Supported fields SHALL include:

- `default-bundle` (optional): the bundle the TUI browses by default.

`default-bundle` is a `ui.toml` key; `users.toml` remains the identity and
policy file and does not carry it.

The runtime SHALL treat a missing `ui.toml` as no configured UI-surface
defaults, not an error. A malformed `ui.toml` SHALL fail fast with a structured
bootstrap validation error, and a malformed overlay `ui.toml` SHALL NOT fall
through to the base file. Loading `ui.toml` SHALL be read-only and SHALL NOT
scaffold or modify configuration artifacts.

`agentmux check configuration` SHALL validate `ui.toml` through the same
read-only loader and the same effective-file lookup, reporting a malformed file
with its path and field-level detail before the relay or TUI would reject it at
startup — matching the pre-flight coverage of `relay.toml` and `users.toml`.

#### Scenario: Resolve default browsing bundle from ui.toml

- **WHEN** the runtime resolves the TUI browsing bundle without `--bundle`
- **AND** `ui.toml` defines `default-bundle`
- **THEN** the runtime resolves the browsing bundle from that `default-bundle`

#### Scenario: Overlay ui.toml shadows the base file

- **WHEN** `ui.toml` exists under both the overlay and the base root
- **THEN** the overlay file supplies the UI-surface defaults

#### Scenario: Treat missing ui.toml as no UI defaults

- **WHEN** no `ui.toml` exists under the overlay or the base root
- **THEN** the runtime resolves no configured `default-bundle`
- **AND** startup proceeds

#### Scenario: Malformed overlay ui.toml does not fall through

- **WHEN** an overlay `ui.toml` exists but cannot be parsed
- **THEN** loading fails with a structured bootstrap validation error
- **AND** the base `ui.toml` is not used
