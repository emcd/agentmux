## MODIFIED Requirements

### Requirement: TUI Sender Association Resolution

The runtime SHALL resolve sender identity for `agentmux tui` and
session-selected `agentmux send` invocations using global `users.toml`
identity configuration and `ui.toml` UI-surface defaults with deterministic
precedence.

Sender/session resolution SHALL be:

1. explicit CLI `--as-session` when present
2. `default-session` from active global `users.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution for interactive `agentmux tui` SHALL be lenient — the operator
selects a browsing bundle in the picker, so an absent default is not an error:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Bundle resolution for session-selected `agentmux send` SHALL be:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active `ui.toml`
3. fail-fast `validation_unknown_bundle`

Association-derived sender fallback SHALL NOT be used for these surfaces.

If selected session resolves to invalid sender identity, runtime SHALL fail with
`validation_unknown_sender`.
If selected session references unknown policy, runtime SHALL fail with
`validation_unknown_policy`.

#### Scenario: Resolve sender and bundle from explicit selectors

- **WHEN** invocation includes `--bundle agentmux --as-session user`
- **THEN** runtime resolves bundle `agentmux` and sender from session `user`

#### Scenario: Resolve sender and bundle from global defaults

- **WHEN** invocation omits selectors
- **AND** `ui.toml` provides `default-bundle` and `users.toml` provides
  `default-session`
- **THEN** runtime resolves bundle/session from those defaults

#### Scenario: Fall back to an available bundle when tui default is missing

- **WHEN** `agentmux tui` omits `--bundle`
- **AND** `default-bundle` is absent in `ui.toml`
- **THEN** runtime resolves the browsing bundle from the first available
  configured bundle, or an empty browsing context when none is available

#### Scenario: Reject send when default bundle is missing

- **WHEN** `agentmux send` omits `--bundle`
- **AND** `default-bundle` is absent in `ui.toml`
- **THEN** runtime returns `validation_unknown_bundle`

### Requirement: TUI Sender Configuration Files

The runtime SHALL support global user session configuration at:

- normal config path: `<config-root>/users.toml`
- debug/testing override path:
  `.auxiliary/configuration/agentmux/overrides/users.toml`

Supported fields SHALL use kebab-case and include:

- `default-session` (optional)
- `[[sessions]]` entries with:
  - required `id` (in `session@GLOBAL` canonical form)
  - required exactly one coder-less marker subtable: `[sessions.ui]` (TUI
    operators) or `[sessions.pubsub]` (embedded agents)
  - optional `name`
  - optional `policy`

`users.toml` is the identity and policy file; UI-surface operational defaults
such as `default-bundle` live in `ui.toml` (see `ui-surface-configuration`).

Global user sessions are coder-less by construction; a `coder` reference is
not accepted in `users.toml` entries.

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.
Session `id` SHALL be in `session@GLOBAL` canonical form and SHALL be unique
within the file.

#### Scenario: Resolve sender from session entry in global users.toml

- **WHEN** runtime selects session `user@GLOBAL`
- **AND** `[[sessions]]` in `users.toml` contains `id = "user@GLOBAL"`
- **THEN** runtime resolves sender identity as `user@GLOBAL`

#### Scenario: Reject unknown configured default session

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code
