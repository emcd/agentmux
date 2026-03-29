## MODIFIED Requirements

### Requirement: TUI Session Override Precedence Hook

`agentmux tui` SHALL support session/bundle selectors:

- optional `--session <session-selector>`
- optional `--bundle <bundle-id>`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Bundle selection SHALL resolve as:

1. explicit `--bundle`
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Session selection SHALL resolve as:

1. explicit `--session`
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Resolved TUI session SHALL provide canonical wire `session-id` for relay
operations in that process.

#### Scenario: Launch TUI with explicit session and bundle selectors

- **WHEN** an operator runs `agentmux tui --bundle agentmux --session user`
- **THEN** startup resolves session `user` on bundle `agentmux`

#### Scenario: Launch TUI from config defaults

- **WHEN** operator runs `agentmux tui` without `--bundle` and `--session`
- **AND** `tui.toml` has `default-bundle` and `default-session`
- **THEN** startup resolves both values from config defaults

#### Scenario: Reject missing default session when selector is omitted

- **WHEN** operator runs `agentmux tui` without `--session`
- **AND** `default-session` is absent from `tui.toml`
- **THEN** CLI fails fast with `validation_unknown_session`

#### Scenario: Reject sender flag on TUI command

- **WHEN** an operator runs `agentmux tui --sender relay`
- **THEN** CLI rejects invocation with `validation_invalid_arguments`

## ADDED Requirements

### Requirement: Send Session Selector Surface

`agentmux send` SHALL support optional sender session selector:

- `--session <session-selector>`

`agentmux send --sender` SHALL NOT be supported in MVP.

Send bundle resolution SHALL be:

1. explicit `--bundle`
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Send session resolution SHALL be:

1. explicit `--session`
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Resolved session `session-id` SHALL be used as send caller identity before
relay dispatch.

#### Scenario: Send with explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --session user --target mcp --message "hi"`
- **AND** session `user` resolves to `session-id = "tui"`
- **THEN** send caller identity resolves as session `tui`

#### Scenario: Send with default session fallback

- **WHEN** an operator runs `agentmux send --target mcp --message "hi"`
- **AND** `default-bundle` is defined in `tui.toml`
- **AND** `default-session` is defined in `tui.toml`
- **THEN** send caller identity resolves from that default session

#### Scenario: Reject missing default bundle for send

- **WHEN** an operator runs `agentmux send --session user --target mcp --message "hi"`
- **AND** `default-bundle` is absent from `tui.toml`
- **THEN** CLI rejects invocation with `validation_unknown_bundle`

#### Scenario: Reject unknown explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --session missing --target mcp --message "hi"`
- **AND** `tui.toml` has no matching `[[sessions]]` selector
- **THEN** CLI rejects invocation with `validation_unknown_session`

#### Scenario: Reject sender flag on send command

- **WHEN** an operator runs `agentmux send --sender relay --target mcp --message "hi"`
- **THEN** CLI rejects invocation with `validation_invalid_arguments`
