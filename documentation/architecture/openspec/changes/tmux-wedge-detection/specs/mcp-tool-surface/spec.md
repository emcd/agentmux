## MODIFIED Requirements

### Requirement: Send Target Selection

Send target identifiers SHALL be:

- bundle member session id
- UI session id (where UI routing is supported)

Configured session `name` values and display-name aliases are not canonical send
target identifiers and SHALL NOT be relay-routed.

If one token matches both a bundle member `session_id` and UI session id, the
bundle member `session_id` interpretation SHALL win.

`send` SHALL accept a transport-specific timeout override field for ACP
targets:

- `acp_turn_timeout_ms` (positive integer milliseconds) for ACP targets

Tmux delivery is configured per coder via the
`[coders.<id>.tmux].prime-timeout-ms` TOML key (see the session-relay
Tmux Prime Timeout requirement); `send` SHALL NOT accept a per-call
tmux timeout override field. v1 of Tmux delivery is config-only.

`send` SHALL reject ACP timeout overrides against non-ACP targets with
`validation_invalid_timeout_field_for_transport`.

`send` authorization scope SHALL follow requester policy control:

- `home`
- `all`

#### Scenario: Reject non-canonical configured-name token for explicit send target

- **WHEN** `send` targets a configured session `name` token
- **THEN** the tool returns `validation_unknown_target`

#### Scenario: Resolve overlap token as bundle member session_id

- **WHEN** one explicit target token matches both bundle member `session_id` and
  UI session id
- **THEN** the token is interpreted as bundle member `session_id`

#### Scenario: Reject ACP timeout override for tmux target

- **WHEN** `send` targets tmux-backed session
- **AND** the request payload includes `acp_turn_timeout_ms`
- **THEN** the tool returns `validation_invalid_timeout_field_for_transport`