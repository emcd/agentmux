## ADDED Requirements

### Requirement: TUI Sender Association Resolution

The TUI runtime SHALL resolve sender association at startup using precedence:

1. explicit CLI `--sender` when present
2. local testing override sender from
   `.auxiliary/configuration/agentmux/overrides/tui.toml` when present in
   debug/testing mode
3. normal config sender from `<config-root>/tui.toml` when present
4. runtime association auto-discovery

If sender cannot be resolved or does not map to a known bundle member, startup
SHALL fail with structured `validation_unknown_sender`.

#### Scenario: Resolve sender from CLI override

- **WHEN** TUI startup includes explicit `--sender`
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from debug/testing override

- **WHEN** CLI sender is absent
- **AND** runtime is debug/testing mode
- **AND** `overrides/tui.toml` provides `sender`
- **THEN** sender association resolves from override sender value

#### Scenario: Resolve sender from normal config tui.toml

- **WHEN** CLI sender is absent
- **AND** override sender is absent or not active for current mode
- **AND** `<config-root>/tui.toml` provides `sender`
- **THEN** sender association resolves from normal config sender value

#### Scenario: Resolve sender from runtime association fallback

- **WHEN** all configured sender files are absent or inapplicable
- **THEN** runtime association fallback is used to resolve sender identity

#### Scenario: Reject unresolved sender association

- **WHEN** all sender resolution sources fail to produce a valid sender
- **THEN** TUI startup returns structured `validation_unknown_sender`

### Requirement: TUI Sender Configuration Files

The TUI runtime SHALL support optional sender configuration files:

- normal config path: `<config-root>/tui.toml`
- debug/testing override path:
  `.auxiliary/configuration/agentmux/overrides/tui.toml`

Supported fields for this proposal SHALL include:

- `sender`

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.

#### Scenario: Ignore missing tui sender config files

- **WHEN** both normal and override `tui.toml` are missing
- **THEN** startup continues using remaining precedence sources

#### Scenario: Reject malformed normal tui.toml

- **WHEN** `<config-root>/tui.toml` exists but has invalid TOML or invalid
  fields
- **THEN** startup fails with structured bootstrap validation error

#### Scenario: Reject malformed override tui.toml in debug/testing mode

- **WHEN** override `tui.toml` is active for mode and malformed
- **THEN** startup fails with structured bootstrap validation error

### Requirement: TUI Override File VCS Posture

TUI local testing override file SHALL follow the existing local override VCS
posture so per-user test defaults do not leak into shared tracked
configuration.

#### Scenario: Keep override tui.toml under ignored overrides directory

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overrides/tui.toml` is covered by
  the existing ignored overrides path
