## MODIFIED Requirements

### Requirement: TUI Sender Identity Precedence

`agentmux tui` SHALL resolve identity and the browsing bundle from TUI
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--as-session` when provided
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Browsing bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Association-derived sender fallback SHALL NOT be used for TUI startup.

TUI runtime SHALL use resolved session `id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit selectors

- **WHEN** operator starts TUI with `--bundle agentmux --as-session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured in active TUI configuration
- **THEN** TUI resolves browsing bundle `agentmux` and sender identity
  `user@GLOBAL`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without explicit selectors
- **AND** `ui.toml` defines `default-bundle` and `users.toml` defines
  `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Allow startup without bundle default

- **WHEN** operator starts TUI without `--bundle`
- **AND** `ui.toml` does not define `default-bundle`
- **THEN** TUI resolves the browsing bundle from the first available configured
  bundle
- **AND** if no bundle is available, TUI starts with an empty browsing context

#### Scenario: Fail when required session selector is absent

- **WHEN** operator starts TUI without `--as-session`
- **AND** `users.toml` does not define `default-session`
- **THEN** startup fails with `validation_unknown_session`

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** `default-session` in `users.toml` references a policy that does not
  exist
- **THEN** startup fails with `validation_unknown_policy`
