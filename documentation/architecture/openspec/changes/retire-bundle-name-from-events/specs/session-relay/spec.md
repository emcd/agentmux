## MODIFIED Requirements

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `target_session`
- `created_at`

`target_session` SHALL carry the canonical `session@bundle` form per the
Canonical Session Identity requirement. `bundle_name` is retired; bundle
context is recoverable from the `target_session` suffix.

Event types SHALL include:

- `incoming_message`
- `delivery_outcome`

`incoming_message` payload SHALL include:

- `message_id`
- `sender_session`
- `body`
- optional `cc_sessions`

`delivery_outcome` payload SHALL include:

- `message_id`
- `phase` (`routed`|`delivered`|`failed`)
- `outcome` (`success`|`timeout`|`failed`|null)
- optional `reason_code`
- optional `reason`

`delivery_outcome` SHALL be the canonical machine completion/update carrier for
stream-path delivery updates and SHALL be keyed by `message_id`.

`phase=routed` SHALL be diagnostic metadata and SHALL set `outcome=null`.

Terminal updates SHALL keep existing external vocabulary:

- delivered terminal: `phase=delivered`, `outcome=success`
- failure terminal: `phase=failed`, `outcome` in (`timeout`|`failed`)

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `phase=failed`
- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Push routed diagnostic update

- **WHEN** relay resolves stream routing for a target delivery
- **THEN** relay pushes `delivery_outcome` with `phase=routed`
- **AND** sets `outcome=null`

#### Scenario: Push terminal delivery outcome update

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical `phase` and `outcome` values

#### Scenario: Map dropped_on_shutdown to failed terminal update

- **WHEN** relay terminal state for a target is `dropped_on_shutdown`
- **THEN** `delivery_outcome` includes `phase=failed`
- **AND** includes `outcome=failed`
- **AND** includes `reason_code=dropped_on_shutdown`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `acp_entries_v1`)

`bundle_name` is retired from look responses; bundle context is recoverable
from the `target_session` suffix.

When `snapshot_format = "lines"`, responses SHALL include:
- `snapshot_lines` (`string[]`)

When `snapshot_format = "acp_entries_v1"`, responses SHALL include:
- `snapshot_entries` (`object[]`) using canonical ACP entry vocabulary.

For ACP targets, successful relay look responses SHALL additionally include:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `entries_total` (`number`, required) — count of all entries available in
  the buffer before windowing; reflects the full buffer on every response,
  including stale and empty snapshots
- `returned_entries_count` (`number`, required) — count of entries in
  `snapshot_entries` after windowing; SHALL equal the length of
  `snapshot_entries`
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when unavailable)

ACP stale reason vocabulary:

- `acp_worker_initializing`
- `acp_worker_unavailable`
- `acp_snapshot_prime_timeout`
- `acp_stream_stalled`

#### Scenario: Return canonical look payload for tmux target

- **WHEN** look succeeds for tmux target
- **THEN** relay returns `snapshot_format="lines"`
- **AND** includes ordered `snapshot_lines` from oldest to newest
- **AND** ACP additive fields are omitted

#### Scenario: Return ACP look payload with structured entries

- **WHEN** look succeeds for ACP target
- **THEN** relay returns `snapshot_format="acp_entries_v1"`
- **AND** includes `snapshot_entries`
- **AND** includes required ACP additive fields `freshness`,
  `snapshot_source`, `entries_total`, and `returned_entries_count`

#### Scenario: Report entry counts for windowed ACP look

- **WHEN** ACP look returns a window narrower than the full buffer
- **THEN** `entries_total` reflects the full buffer count
- **AND** `returned_entries_count` equals the length of `snapshot_entries`
- **AND** `returned_entries_count <= entries_total`

#### Scenario: Keep required ACP fields when snapshot is empty

- **WHEN** ACP look succeeds with `snapshot_entries=[]`
- **THEN** relay still includes required `freshness`, `snapshot_source`,
  `entries_total`, and `returned_entries_count`
- **AND** `returned_entries_count=0`
