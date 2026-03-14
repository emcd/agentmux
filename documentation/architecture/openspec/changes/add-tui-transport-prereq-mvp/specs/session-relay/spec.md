## ADDED Requirements

### Requirement: Relay TUI Event Retrieval Operation

The system SHALL provide a relay-level read operation for TUI update retrieval:
`events`.

`events` request fields SHALL include:

- `requester_session` (required)
- `since_event_id` (optional)
- `limit` (optional positive integer)
- `bundle_name` (optional/redundant when bundle is already bound by
  association/socket context)

`events` response fields SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `events` (`Event[]`)
- `next_event_id` (nullable)

`limit` bounds SHALL be deterministic:

- default `limit = 100`
- maximum `limit = 1000`
- valid range `1..=1000`

#### Scenario: Retrieve events in associated bundle context

- **WHEN** caller requests `events` without `bundle_name`
- **THEN** relay resolves associated bundle context
- **AND** returns canonical event payload array for requester session

#### Scenario: Reject mismatched bundle scope for events in MVP

- **WHEN** caller requests `events` with `bundle_name` outside associated
  bundle context
- **THEN** relay rejects request with `validation_cross_bundle_unsupported`

#### Scenario: Reject invalid events limit

- **WHEN** caller provides `limit` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_event_limit`

### Requirement: Relay TUI Event Payload Union

Relay `events` payload entries SHALL use a canonical event union with
`event_type` values:

- `incoming_message`
- `delivery_outcome`

Every event entry SHALL include:

- `event_id`
- `event_type`
- `created_at`

For `incoming_message`, event payload SHALL include:

- `message_id`
- `sender_session`
- `target_session`
- `body`
- `cc_sessions` (optional `string[]`)

For `delivery_outcome`, event payload SHALL include:

- `message_id`
- `target_session`
- `outcome` (`success`|`timeout`|`failed`)
- `reason_code` (nullable)
- `reason` (nullable)

#### Scenario: Emit canonical incoming message event

- **WHEN** relay has message delivery content for requester session consumption
- **THEN** `events` response entry uses `event_type=incoming_message`
- **AND** includes canonical incoming message payload fields

#### Scenario: Emit canonical terminal delivery outcome event

- **WHEN** relay records terminal delivery state for a message target
- **THEN** `events` response entry uses `event_type=delivery_outcome`
- **AND** outcome is one of `success`, `timeout`, or `failed`

### Requirement: Event Cursor and Reconnect Semantics

Relay `events` retrieval SHALL support deterministic resume semantics using
`since_event_id`.

If `since_event_id` is unknown/expired for current in-memory event window,
relay SHALL fail fast with stable validation error rather than silently
returning misleading partial history.

#### Scenario: Resume from known cursor

- **WHEN** caller requests `events` with a known `since_event_id`
- **THEN** relay returns events strictly after that cursor
- **AND** returns `next_event_id` for subsequent resume requests

#### Scenario: Fail fast on unknown cursor

- **WHEN** caller requests `events` with unknown or expired `since_event_id`
- **THEN** relay rejects request with stable validation error code
- **AND** does not silently reset cursor to latest
