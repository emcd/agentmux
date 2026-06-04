## MODIFIED Requirements

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required)
- `lines` (optional)
- `offset` (optional; default `0`) — for ACP targets, pages the entry
  window backward from the newest end; for tmux targets only `0` is valid
- `bundle_name` (optional/redundant when bundle is already bound by
  association/socket context)

MVP authorization posture for `look` SHALL be:

- default scope `self`
- broader scope controlled by policy (`all:home` or `all:all`)
- cross-bundle look currently unsupported by runtime contract

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request omits `bundle_name`
- **THEN** relay resolves bundle from associated runtime context

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Reject mismatched bundle name in MVP

- **WHEN** look request includes `bundle_name` that does not match
  associated runtime context
- **THEN** relay rejects request with `validation_cross_bundle_unsupported`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Reject nonzero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is present and not equal to `0`
- **THEN** relay rejects request with `validation_offset_unsupported`

#### Scenario: Accept zero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is omitted or equal to `0`
- **THEN** relay accepts request and proceeds with the look operation

### Requirement: Look Capture Window Bounds

Look capture window SHALL use deterministic bounds, with the default
keyed on target type:

- default `lines = 120` for tmux targets
- default `lines = 50` for ACP targets
- maximum `lines = 1000`
- valid range `1..=1000`

For ACP targets, the entry window SHALL be selected from the newest end of
the available buffer using `offset`:

- the window is the half-open range `[start, end)` over the full ordered
  entry buffer, where `end = entries_total.saturating_sub(offset)` and
  `start = end.saturating_sub(lines)` (two saturating subtractions, no other
  arithmetic)
- when `offset >= entries_total`, the window SHALL be empty
  (`returned_entries_count = 0`); this is a normal terminal page, NOT an error

#### Scenario: Apply default tmux line window

- **WHEN** look request for a tmux target omits `lines`
- **THEN** relay captures using default `lines = 120`

#### Scenario: Apply default ACP entry window

- **WHEN** look request for an ACP target omits `lines`
- **THEN** relay windows using default `lines = 50`

#### Scenario: Reject out-of-range line window

- **WHEN** look request includes `lines` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_lines`

#### Scenario: Window ACP entries backward from newest end

- **WHEN** ACP look request includes `offset` with `offset < entries_total`
- **THEN** relay returns the window ending `offset` entries back from the
  newest entry, sized by `lines`

#### Scenario: Offset beyond available entries yields empty window

- **WHEN** ACP look request includes `offset >= entries_total`
- **THEN** relay returns `snapshot_entries=[]` with `returned_entries_count=0`
- **AND** relay does NOT return an error
- **AND** `entries_total` still reflects the full buffer count

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `acp_entries_v1`)

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

ACP stale reason vocabulary SHALL be fixed in MVP:

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
