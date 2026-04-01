## ADDED Requirements

### Requirement: Permission Decision Capability Contract

Relay SHALL evaluate ACP permission-request decision authority using policy
capability `grant`.

For MVP:

- allowed values: `none`, `all:home`
- default when omitted: `none`
- invalid values (`self`, `all:all`, unknown values) SHALL fail validation with
  `validation_invalid_policy_scope`

#### Scenario: Reject invalid grant scope self

- **WHEN** policy configuration sets `grant = "self"`
- **THEN** relay rejects configuration with `validation_invalid_policy_scope`

#### Scenario: Default omitted grant to none

- **WHEN** policy omits `grant`
- **THEN** relay treats `grant` as `none`

### Requirement: UI-Mediated Decision Submitter Gate

Permission decision actions (`approve`/`deny`) SHALL be accepted only from
associated principals with `client_class=ui`.

#### Scenario: Reject non-ui decision submitter

- **WHEN** an associated principal with `client_class=agent` submits
  `permission.approve`
- **THEN** relay rejects with `validation_invalid_client_class_for_action`

### Requirement: Non-Spoofable Decision Actor Identity

Relay SHALL derive permission decision actor identity from association/request
context and SHALL NOT trust caller-supplied identity fields in action payload.

#### Scenario: Reject caller-supplied decision actor field

- **WHEN** `permission.deny` payload includes `ui_session_id`
- **THEN** relay rejects with `validation_invalid_params`

### Requirement: Same-Bundle Permission Decision Scope

Permission request routing and decisioning SHALL be same-bundle only in MVP.
Cross-bundle routing/decision attempts SHALL be rejected with
`validation_cross_bundle_unsupported`.

#### Scenario: Reject cross-bundle permission decision attempt

- **WHEN** a decision action targets a permission request owned by another bundle
- **THEN** relay rejects with `validation_cross_bundle_unsupported`

### Requirement: Bounded Permission Queue and Replay

Relay SHALL queue ACP permission requests when no grant-authorized UI is
connected.

Queue contract:

- bundle-scoped global FIFO ordering by `(enqueued_at, permission_request_id)`
- `max_pending` default `256`
- optional `[relay.permission] max-pending` override in `1..4096`
- enqueue beyond bound SHALL fail with `runtime_permission_queue_full`

Connect/reconnect replay contract:

- relay emits `permission.snapshot` first
- relay then replays pending `permission.requested` oldest->newest
- replay is at-least-once; consumers dedupe by `permission_request_id`

#### Scenario: Reject enqueue beyond queue bound

- **WHEN** pending queue depth equals effective `max_pending`
- **AND** another permission request is queued
- **THEN** relay fails with `runtime_permission_queue_full`

#### Scenario: Emit snapshot then replay on authorized ui connect

- **WHEN** grant-authorized UI connects
- **THEN** relay emits `permission.snapshot` before replay
- **AND** replays pending requests in FIFO order

### Requirement: Durable Pending Queue Restoration

Relay SHALL persist pending permission queue state across restart.
If persisted state is unreadable or corrupt, relay SHALL fail fast with
`runtime_permission_queue_unavailable` for that bundle and SHALL NOT silently
drop pending items.

#### Scenario: Fail fast on unrecoverable queue state

- **WHEN** relay startup cannot restore pending permission queue state
- **THEN** relay fails with `runtime_permission_queue_unavailable`

### Requirement: Non-Expiring Permission Pending Lifecycle (MVP)

MVP permission requests SHALL be non-expiring while relay and worker state
remain healthy.

Pending requests SHALL remain pending until one of:

- explicit authorized `approve` decision
- explicit authorized `deny` decision
- hard terminal cancellation condition (for example session/worker termination
  or aborted permission wait)

Relay SHALL NOT apply timer-based auto-expiry for permission requests in MVP.
ACP send turn-timeout fields (`acp_turn_timeout_ms`, `[coders.acp] turn-timeout-ms`)
remain independent from permission decision lifecycle.

#### Scenario: Keep permission request pending without timer expiry

- **WHEN** permission request is queued and no decision is made
- **AND** relay/worker remain healthy
- **THEN** request remains pending and is not auto-expired by timer

### Requirement: Permission Lifecycle Event Carrier

Relay stream events SHALL be canonical machine carrier for permission
lifecycle.

Required event names:

- `permission.snapshot`
- `permission.requested`
- `permission.resolved`

Required correlation keys on lifecycle events:

- `message_id`
- `permission_request_id`

Required minimum event fields:

- `permission.requested`: `message_id`, `permission_request_id`,
  `target_session`, `requested_kind`, `requested_details`, `enqueued_at`
- `permission.resolved`: `message_id`, `permission_request_id`, `outcome`,
  `reason_code`, `decided_by`, `resolved_at`

Inscriptions MAY be emitted but SHALL be additive only.

#### Scenario: Emit canonical resolved event with correlation keys

- **WHEN** permission request reaches terminal resolution
- **THEN** relay emits `permission.resolved`
- **AND** event includes `message_id` and `permission_request_id`

### Requirement: Permission Resolution and Enforcement Mapping

Relay SHALL enforce permission terminal outcomes with deterministic mapping to
ACP action and sender-visible terminal outcome/reason_code.

Mapping:

- `approved` -> send ACP allow; prompt continues under existing ACP stop-reason
  mapping contract
- `runtime_permission_request_denied` -> send ACP deny; sender-visible terminal
  outcome `failed` with `reason_code=runtime_permission_request_denied`
- `runtime_permission_request_cancelled` -> abort pending ACP wait/turn;
  sender-visible terminal outcome `failed` with
  `reason_code=runtime_permission_request_cancelled`

For sync phase-1 responses already returned with
`details.delivery_phase = "accepted_in_progress"`, relay SHALL keep phase-1
response immutable.

#### Scenario: Map denied permission to failed terminal outcome

- **WHEN** permission decision resolves to denied
- **THEN** relay sends ACP deny
- **AND** sender-visible terminal outcome is `failed`
- **AND** `reason_code = runtime_permission_request_denied`

#### Scenario: Keep sync phase-1 response immutable after later permission cancellation

- **WHEN** relay already returned sync phase-1 with
  `details.delivery_phase = "accepted_in_progress"`
- **AND** permission later resolves to cancelled
- **THEN** relay does not mutate the earlier phase-1 response

### Requirement: Permission Decision Arbitration

First authorized UI decision SHALL win.
Subsequent decisions on resolved requests SHALL be rejected with
`runtime_permission_request_already_resolved` and SHALL NOT mutate state.

#### Scenario: Reject late decision after prior approval

- **WHEN** a second UI submits decision for already approved request
- **THEN** relay rejects with `runtime_permission_request_already_resolved`

### Requirement: Permission Decision Denial Schema

When relay denies permission decisioning by policy, relay SHALL return
`authorization_forbidden` with canonical minimum details:

- `capability`
- `requester_session`
- `bundle_name`
- `reason`

Optional additive details MAY include `target_session`, `targets`,
`policy_rule_id`, and ACP-specific metadata.

#### Scenario: Return canonical denial details for unauthorized ui decision

- **WHEN** UI principal lacks `grant` permission
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include canonical required fields
