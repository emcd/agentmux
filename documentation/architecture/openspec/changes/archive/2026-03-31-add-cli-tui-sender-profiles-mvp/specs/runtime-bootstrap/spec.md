## MODIFIED Requirements

### Requirement: TUI Sender Association Resolution

The runtime SHALL resolve sender identity for `agentmux tui` and
session-selected `agentmux send` invocations using global `tui.toml`
configuration with deterministic precedence.

Sender/session resolution SHALL be:

1. explicit CLI `--session` when present
2. `default-session` from active global `tui.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution for `agentmux tui` SHALL be:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Bundle resolution for session-selected `agentmux send` SHALL be:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Association-derived sender fallback SHALL NOT be used for these surfaces in
MVP.

If selected session resolves to invalid sender identity, runtime SHALL fail with
`validation_unknown_sender`.
If selected session references unknown policy, runtime SHALL fail with
`validation_unknown_policy`.

#### Scenario: Resolve sender and bundle from explicit selectors

- **WHEN** invocation includes `--bundle agentmux --session user`
- **THEN** runtime resolves bundle `agentmux` and sender from session `user`

#### Scenario: Resolve sender and bundle from global defaults

- **WHEN** invocation omits selectors
- **AND** `tui.toml` provides `default-bundle` and `default-session`
- **THEN** runtime resolves bundle/session from those defaults

#### Scenario: Reject startup when default bundle is missing

- **WHEN** `agentmux tui` omits `--bundle`
- **AND** `default-bundle` is absent in `tui.toml`
- **THEN** runtime returns `validation_unknown_bundle`

#### Scenario: Reject send when default bundle is missing

- **WHEN** `agentmux send` omits `--bundle`
- **AND** `default-bundle` is absent in `tui.toml`
- **THEN** runtime returns `validation_unknown_bundle`

### Requirement: TUI Sender Configuration Files

The runtime SHALL support global TUI session configuration files at:

- normal config path: `<config-root>/tui.toml`
- debug/testing override path:
  `.auxiliary/configuration/agentmux/overrides/tui.toml`

Supported fields SHALL use kebab-case and include:

- `default-bundle` (optional)
- `default-session` (optional)
- `[[sessions]]` entries with required:
  - `id`
  - `policy`
- `[[sessions]]` optional:
  - `name`

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.
Session `id` SHALL be canonical wire identity for relay operations and SHALL be
unique within the file.

#### Scenario: Resolve sender from session entry in global tui.toml

- **WHEN** runtime selects session `user`
- **AND** `[[sessions]]` contains `id = "user"`
- **THEN** runtime resolves sender identity as `user`

#### Scenario: Reject unknown configured default session

- **WHEN** `default-session = "missing"`
- **AND** no `[[sessions]]` entry has `id = "missing"`
- **THEN** runtime returns `validation_unknown_session`

#### Scenario: Reject duplicate session identifiers in tui.toml

- **WHEN** `tui.toml` contains multiple `[[sessions]]` entries with the same
  `id`
- **THEN** runtime fails fast with structured bootstrap validation error

#### Scenario: Reject selected session with unknown policy reference

- **WHEN** runtime selects session `user`
- **AND** `[[sessions]]` entry references unknown `policy`
- **THEN** runtime returns `validation_unknown_policy`
