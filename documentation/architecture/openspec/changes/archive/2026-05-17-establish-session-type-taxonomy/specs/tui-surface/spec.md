## MODIFIED Requirements

### Requirement: TUI Sender Identity Precedence

`agentmux tui` SHALL resolve identity and bundle from global `users.toml`
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--session` when provided
2. `default-session` from global `users.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from global `users.toml`
3. fail-fast `validation_unknown_bundle`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Association-derived sender fallback SHALL NOT be used for TUI startup in MVP.

TUI runtime SHALL use resolved session `id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit session/bundle selectors

- **WHEN** operator starts TUI with `--bundle agentmux --session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured in global users
- **THEN** TUI resolves bundle `agentmux` and sender identity `user@GLOBAL`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without `--bundle`/`--session`
- **AND** global `users.toml` defines `default-bundle` and `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Reject sender flag at startup

- **WHEN** operator starts TUI with `--sender relay`
- **THEN** startup fails with a stable validation error

#### Scenario: Fail when required defaults absent

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** `default-session` in `users.toml` references a policy that does not
  exist
- **THEN** startup fails with `validation_unknown_policy`

## ADDED Requirements

### Requirement: TUI Session Type Validation

`agentmux tui --as-session X` SHALL fail fast when session `X` is not
configured with session type `ui`.

If the resolved session has any other type (`tmux`, `acp`, `pubsub`), the TUI
SHALL reject startup with a structured validation error rather than proceeding
with an incompatible delivery model.

#### Scenario: Reject --as-session with non-ui session type

- **WHEN** operator starts TUI with `--as-session relay`
- **AND** session `relay` is a coder-backed session (resolved type `tmux`)
- **THEN** startup fails with a structured validation error indicating type
  mismatch

#### Scenario: Accept --as-session with ui session type

- **WHEN** operator starts TUI with `--as-session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured with `[sessions.ui]`
- **THEN** TUI startup proceeds normally
