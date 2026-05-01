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
- ACP permission `options` for explicit operator selection

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

TUI SHALL expose deterministic decision actions keyed by
`permission_request_id`.

Action payload contract:

- `permission.resolve { permission_request_id, outcome, option_id? }`
- allowed outcomes are `selected` and `cancelled`
- `selected` requires `option_id`
- `cancelled` must omit `option_id`

TUI SHALL NOT send caller-supplied actor identity fields in action payload.

#### Scenario: Submit selected action without actor spoof fields

- **WHEN** operator chooses a permission option from pending request
- **THEN** TUI submits `permission.resolve` with
  `permission_request_id`, `outcome=selected`, and explicit `option_id`

#### Scenario: Submit cancelled action without option id

- **WHEN** operator cancels a pending permission request
- **THEN** TUI submits `permission.resolve` with
  `permission_request_id` and `outcome=cancelled`

### Requirement: Session-Scoped Permission Workflow

TUI SHALL provide a session-scoped permission workflow in the Look context for
the active target session.

Workflow contract:

- pending rows in Look are filtered to the active look target session
- selection for multiple pending requests is deterministic by relay FIFO order
- action hints and empty-state text are visible in Look context

#### Scenario: Show session-scoped pending requests in Look

- **WHEN** operator opens Look for target session `acp`
- **AND** pending permissions exist for sessions `acp` and `relay`
- **THEN** Look permission actions render only pending requests for `acp`

### Requirement: Permission Terminal State Updates

TUI SHALL apply terminal updates from `permission.resolved` and remove pending
entries deterministically by `permission_request_id`.

TUI-facing terminal vocabulary SHOULD align to:

- `selected`
- `cancelled`

#### Scenario: Remove pending item on resolved event

- **WHEN** relay emits `permission.resolved` for pending request
- **THEN** TUI marks terminal status and clears pending row for that id
