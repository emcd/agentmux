## MODIFIED Requirements
### Requirement: Send Target Selection

`send` SHALL support exactly one target mode per request:

- `targets` (non-empty list of recipient identifiers)
- `broadcast=true` for full bundle delivery

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

#### Scenario: Reject conflicting timeout override fields

- **WHEN** a caller provides `quiescence_timeout_ms` and
  `acp_turn_timeout_ms` in one request
- **THEN** the system rejects with `validation_conflicting_timeout_fields`

#### Scenario: Reject tmux timeout field on ACP send target

- **WHEN** request resolves target transport as ACP
- **AND** caller provides `quiescence_timeout_ms`
- **THEN** the system rejects with
  `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout field on tmux send target

- **WHEN** request resolves target transport as tmux
- **AND** caller provides `acp_turn_timeout_ms`
- **THEN** the system rejects with
  `validation_invalid_timeout_field_for_transport`

## ADDED Requirements
### Requirement: MCP ACP Sync Delivery-Phase Passthrough

For sync `send` targeting ACP transport, MCP SHALL propagate relay-authored
phase-1 acknowledgment details without adapter mutation.

When relay marks early delivery acknowledgment, MCP response SHALL preserve:

- `outcome = delivered`
- `details.delivery_phase = "accepted_in_progress"`
- unchanged `message_id` for request tracing

#### Scenario: Preserve early delivery-phase marker in MCP sync response

- **WHEN** relay returns sync ACP result with
  `details.delivery_phase = "accepted_in_progress"`
- **THEN** MCP returns the same result fields unchanged
- **AND** retains the same `message_id` in response payload
