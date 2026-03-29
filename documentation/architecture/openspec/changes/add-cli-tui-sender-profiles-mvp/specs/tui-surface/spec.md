## MODIFIED Requirements

### Requirement: TUI Sender Identity Precedence

`agentmux tui` SHALL resolve identity and bundle from global `tui.toml`
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--session` when provided
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Association-derived sender fallback SHALL NOT be used for TUI startup in MVP.

TUI runtime SHALL use resolved session `session-id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit session/bundle selectors

- **WHEN** operator starts TUI with `--bundle agentmux --session user`
- **AND** session `user` resolves to `session-id = "tui"`
- **THEN** TUI resolves bundle `agentmux` and sender identity `tui`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without `--bundle`/`--session`
- **AND** global `tui.toml` defines `default-bundle` and `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Reject sender flag at startup

- **WHEN** operator starts TUI with `--sender relay`
- **THEN** startup fails with `validation_invalid_arguments`

#### Scenario: Fail fast when required defaults are missing

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `tui.toml`
- **THEN** startup fails with stable validation code

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** defaults resolve to session `user`
- **AND** session `user` references unknown policy
- **THEN** startup fails with `validation_unknown_policy`
