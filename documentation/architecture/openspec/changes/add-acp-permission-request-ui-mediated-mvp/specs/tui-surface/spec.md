## ADDED Requirements

### Requirement: TUI Pending Permission Visibility

TUI SHALL expose pending ACP permission requests received from canonical relay
lifecycle events.

Pending list entries SHALL be keyed by `permission_request_id` and include
request context sufficient for operator decisioning, including:

- `message_id`
- `target_session`
- `requested_kind`
- `requested_details`
- `enqueued_at`

#### Scenario: Render pending request from relay permission event

- **WHEN** relay emits `permission.requested`
- **THEN** TUI adds or updates a pending row keyed by `permission_request_id`

### Requirement: Snapshot and Replay Dedupe Contract

On connect/reconnect, TUI SHALL consume `permission.snapshot` plus replayed
`permission.requested` events using dedupe by `permission_request_id` so
at-least-once replay does not create duplicate pending rows.

#### Scenario: Avoid duplicate pending rows after snapshot replay

- **WHEN** TUI receives `permission.snapshot`
- **AND** relay replays matching `permission.requested` events
- **THEN** TUI keeps one pending row per `permission_request_id`

### Requirement: TUI Permission Decision Actions

TUI SHALL expose deterministic approve/deny actions keyed by
`permission_request_id`.

Action payload contract:

- `permission.approve { permission_request_id }`
- `permission.deny { permission_request_id, reason? }`

TUI SHALL NOT send caller-supplied actor identity fields in action payload.

#### Scenario: Submit approve action without actor spoof fields

- **WHEN** operator approves pending permission request
- **THEN** TUI submits `permission.approve` with only
  `permission_request_id`

### Requirement: Permission Terminal State Updates

TUI SHALL apply terminal updates from `permission.resolved` and remove pending
entries deterministically by `permission_request_id`.

TUI-facing terminal vocabulary SHOULD align to:

- `approved`
- `denied`
- `cancelled`

#### Scenario: Remove pending item on resolved event

- **WHEN** relay emits `permission.resolved` for pending request
- **THEN** TUI marks terminal status and clears pending row for that id
