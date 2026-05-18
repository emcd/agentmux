## MODIFIED Requirements

### Requirement: Send Target Selection

Send target identifiers SHALL be:

- bundle member session id
- UI session id (where UI routing is supported)

Configured session `name` values and display-name aliases are not canonical send
target identifiers and SHALL NOT be relay-routed.

If one token matches both a bundle member `session_id` and UI session id, the
bundle member `session_id` interpretation SHALL win.

`send` timeout override fields SHALL be transport-specific:

- `quiescence_timeout_ms` (positive integer milliseconds) for tmux targets
- `acp_turn_timeout_ms` (positive integer milliseconds) for ACP targets

`send` SHALL reject conflicting timeout overrides in one request with
`validation_conflicting_timeout_fields`.

Transport-incompatible timeout overrides SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

`send` SHALL reject any request that includes an unrecognised field with
`validation_invalid_params`. The `delivery_mode` field is explicitly rejected
under this rule.

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

#### Scenario: Reject delivery_mode field

- **WHEN** a caller invokes `send` with a `delivery_mode` field
- **THEN** the tool returns `validation_invalid_params`
- **AND** does not process the request

### Requirement: Send Response Contract

`send` SHALL return a response containing:

- `schema_version`
- `bundle_name`
- `request_id` (when provided by caller)
- `sender_session`
- `sender_display_name` (optional)
- `status`
- `results` (per-target entries)

`status` SHALL be `accepted` and each per-target result SHALL include:

- `target_session`
- `message_id`
- `outcome` = `queued`

#### Scenario: Return accepted outcome for send request

- **WHEN** a caller invokes `send`
- **THEN** the response status is `accepted`
- **AND** per-target outcomes are `queued`

#### Scenario: Return empty results for zero effective recipients

- **WHEN** a caller invokes `send`
- **AND** effective target resolution yields zero recipients
- **THEN** the response includes `results=[]`
- **AND** `status` is `accepted`

## REMOVED Requirements

### Requirement: MCP ACP Sync Delivery-Phase Passthrough
