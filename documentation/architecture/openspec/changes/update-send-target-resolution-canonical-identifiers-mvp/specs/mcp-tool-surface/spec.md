## MODIFIED Requirements

### Requirement: Send Target Selection

`send` SHALL support exactly one target mode per request:

- `targets` (non-empty list of canonical recipient identifiers)
- `broadcast=true` for full bundle delivery

For send explicit targets, canonical identifiers in MVP are:

- bundle member `session_id`,
- UI session id (where UI routing is supported).

Configured session `name` values and display-name aliases are not canonical send
target identifiers and SHALL NOT be relay-routed.

If one token matches both a bundle member `session_id` and UI session id, the
bundle member `session_id` interpretation SHALL win.

`send` SHALL additionally support optional `delivery_mode` with values:

- `async`
- `sync`

If `delivery_mode` is omitted, the system SHALL default to `async`.

`send` timeout override fields SHALL be transport-specific:

- `quiescence_timeout_ms` (positive integer milliseconds) for tmux targets
- `acp_turn_timeout_ms` (positive integer milliseconds) for ACP targets

`send` SHALL reject conflicting timeout overrides in one request with
`validation_conflicting_timeout_fields`.

Transport-incompatible timeout overrides SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

`send` authorization scope SHALL follow requester policy control:

- `all:home`
- `all:all`

#### Scenario: Reject non-canonical configured-name token for explicit send target

- **WHEN** `send` targets a configured session `name` token
- **THEN** the tool returns `validation_unknown_target`

#### Scenario: Resolve overlap token as bundle member session_id

- **WHEN** one explicit target token matches both bundle member `session_id` and
  UI session id
- **THEN** the token is interpreted as bundle member `session_id`

### Requirement: Relay Error Mapping and Validation Semantics

For relay-backed tool calls, MCP SHALL preserve canonical relay error codes for:

- validation failures,
- authorization denials (`authorization_forbidden`),
- runtime/internal failures (as relay-authored runtime error payloads).

For `authorization_forbidden`, `details` SHALL include:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

Validation failures SHALL be returned before authorization denials.

#### Scenario: Unknown target error

- **WHEN** `send` targets a token that is not a canonical send target identifier
- **THEN** the tool returns error code `validation_unknown_target`
- **AND** includes a human-readable message

#### Scenario: Return canonical authorization denial schema

- **WHEN** request is valid/resolved but denied by policy
- **THEN** the tool returns `authorization_forbidden`
- **AND** details include the required denial fields
