## ADDED Requirements

### Requirement: TUI Sender Association Resolution

The TUI runtime SHALL resolve sender association at startup using precedence:

1. explicit CLI `--sender` when present
2. TUI config `sender` from `<config-root>/tui.toml` when present
3. runtime association auto-discovery

If sender cannot be resolved or does not map to a known bundle member, startup
SHALL fail with structured `validation_unknown_sender`.

#### Scenario: Resolve sender from CLI override

- **WHEN** TUI startup includes explicit `--sender`
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from tui.toml default

- **WHEN** CLI sender is absent
- **AND** `<config-root>/tui.toml` provides `sender`
- **THEN** sender association resolves to configured default sender

#### Scenario: Resolve sender from runtime association fallback

- **WHEN** CLI sender is absent
- **AND** `tui.toml` sender is absent
- **THEN** runtime association fallback is used to resolve sender identity

#### Scenario: Reject unresolved sender association

- **WHEN** all sender resolution sources fail to produce a valid sender
- **THEN** TUI startup returns structured `validation_unknown_sender`

### Requirement: Local TUI Configuration File

The TUI runtime SHALL support optional local TUI config in:

- `<config-root>/tui.toml`

Supported fields for this proposal SHALL include:

- `sender`

Missing `tui.toml` SHALL not be treated as an error.
Malformed `tui.toml` SHALL fail fast with structured bootstrap validation
errors.

#### Scenario: Ignore missing tui.toml

- **WHEN** `<config-root>/tui.toml` does not exist
- **THEN** startup continues using CLI and association resolution

#### Scenario: Reject malformed tui.toml

- **WHEN** `<config-root>/tui.toml` exists but has invalid TOML or invalid
  fields
- **THEN** startup fails with structured bootstrap validation error
