## MODIFIED Requirements

### Requirement: TUI Sender Override Precedence Hook

`agentmux tui` SHALL support session/bundle selectors:

- optional `--as-session <session-selector>`
- optional `--bundle <bundle-id>`

Bundle selection for interactive `agentmux tui` SHALL be lenient — the operator
picks a browsing bundle in the picker, so an absent default is not an error:

1. explicit `--bundle`
2. `default-bundle` from `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Session selection SHALL resolve as:

1. explicit `--as-session`
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Resolved TUI session SHALL provide canonical wire `id` for relay
operations in that process.

#### Scenario: Launch TUI with explicit session and bundle selectors

- **WHEN** an operator runs `agentmux tui --bundle agentmux --as-session user`
- **THEN** startup resolves session `user` on bundle `agentmux`

#### Scenario: Launch TUI from config defaults

- **WHEN** operator runs `agentmux tui` without `--bundle` and `--as-session`
- **AND** `ui.toml` defines `default-bundle` and `users.toml` defines
  `default-session`
- **THEN** startup resolves both values from config defaults

#### Scenario: Reject missing default session when selector is omitted

- **WHEN** operator runs `agentmux tui` without `--as-session`
- **AND** `default-session` is absent from `users.toml`
- **THEN** CLI fails fast with `validation_unknown_session`

#### Scenario: Fall back to an available bundle when tui default is absent

- **WHEN** operator runs `agentmux tui` without `--bundle`
- **AND** `default-bundle` is absent from `ui.toml`
- **THEN** startup resolves the browsing bundle from the first available
  configured bundle, or an empty browsing context when none is available

### Requirement: Send Session Selector Surface

`agentmux send` SHALL support optional sender session selector:

- `--as-session <session-selector>`

Send bundle resolution SHALL be:

1. explicit `--bundle`
2. `default-bundle` from `ui.toml`
3. fail-fast `validation_unknown_bundle`

Send session resolution SHALL be:

1. explicit `--as-session`
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Resolved session `id` SHALL be used as send caller identity before
relay dispatch.

#### Scenario: Send with explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session user --target mcp --message "hi"`
- **AND** session `user` is configured in global TUI sessions
- **THEN** send caller identity resolves as session `user`

#### Scenario: Send with default session fallback

- **WHEN** an operator runs `agentmux send --target mcp --message "hi"`
- **AND** `default-bundle` is defined in `ui.toml`
- **AND** `default-session` is defined in `users.toml`
- **THEN** send caller identity resolves from that default session

#### Scenario: Reject missing default bundle for send

- **WHEN** an operator runs `agentmux send --as-session user --target mcp --message "hi"`
- **AND** `default-bundle` is absent from `ui.toml`
- **THEN** CLI rejects invocation with `validation_unknown_bundle`

#### Scenario: Reject unknown explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session missing --target mcp --message "hi"`
- **AND** `users.toml` has no matching `[[sessions]]` selector
- **THEN** CLI rejects invocation with `validation_unknown_session`
