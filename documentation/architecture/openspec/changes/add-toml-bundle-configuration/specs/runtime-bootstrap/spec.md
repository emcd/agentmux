## MODIFIED Requirements

### Requirement: XDG Configuration Root

The system SHALL resolve default configuration root as:

- debug builds: repository-local
  `.auxiliary/configuration/tmuxmux/` when that directory exists
- otherwise: `$XDG_CONFIG_HOME/tmuxmux` or `~/.config/tmuxmux`

Explicit configuration path overrides (CLI or local override file fields) SHALL
continue to take precedence over default file resolution.

#### Scenario: Use repository-local config root in debug build

- **WHEN** runtime is debug/development mode
- **AND** `.auxiliary/configuration/tmuxmux/` exists under workspace root
- **AND** no explicit config path override is provided
- **THEN** bundle loading uses that repository-local config root

#### Scenario: Ignore repository-local file in release build

- **WHEN** runtime is non-debug/release mode
- **AND** `.auxiliary/configuration/tmuxmux/` exists
- **AND** no explicit config path override is provided
- **THEN** bundle loading uses XDG/home configuration resolution

#### Scenario: Explicit config override takes precedence

- **WHEN** runtime startup receives an explicit config path override
- **THEN** bundle loading uses the explicit path
- **AND** default debug/release config path logic is bypassed

## ADDED Requirements

### Requirement: Bundle Configuration File Name

Bundle configuration SHALL be stored as:

- `coders.toml`
- `bundles/<bundle-id>.toml`

Per-bundle `bundles/<bundle-name>.json` files SHALL NOT be required.

#### Scenario: Load bundle from per-bundle TOML plus coders TOML

- **WHEN** runtime resolves configuration defaults or explicit config path
- **THEN** bundle lookup reads `bundles/<bundle-id>.toml`
- **AND** coder lookup reads `coders.toml`

#### Scenario: Fail when bundle file is absent

- **WHEN** requested bundle ID does not have matching
  `bundles/<bundle-id>.toml`
- **THEN** startup returns structured `validation_unknown_bundle`
