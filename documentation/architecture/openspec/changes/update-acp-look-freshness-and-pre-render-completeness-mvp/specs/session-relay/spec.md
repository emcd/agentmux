## MODIFIED Requirements

### Requirement: ACP Look Snapshot Contract

Relay look SHALL support ACP-backed target sessions using relay-managed
snapshot state populated from a shared per-target ACP worker/client.

For ACP targets, relay SHALL:
- use the same shared per-target ACP worker/client used by ACP send lifecycle
  and prompt execution,
- use one authoritative relay ingestion/write path for ACP look snapshots,
- ingest replay content from `session/load` as baseline snapshot replacement,
- ingest replay content from live `session/update` as append in ACP receive
  order,
- preserve source order (oldest -> newest) without dedupe in MVP,
- retain at most 1000 ACP snapshot entries per session,
- evict oldest entries first when retention exceeds 1000,
- return look results ordered oldest -> newest,
- avoid spawning a second ACP client for steady-state look requests.

Canonical ACP snapshot entry vocabulary SHALL be:
- `kind = "user"` with `lines: string[]`
- `kind = "agent"` with `lines: string[]`
- `kind = "cognition"` with `lines: string[]`
- `kind = "invocation"` with `invocation: object` (pass-through tool-call
  structure)
- `kind = "result"` with `result: object` (pass-through tool-result structure)
- `kind = "update"` with `update_kind: string`, `lines: string[]` for fallback
  unknown/unsupported updates (MUST NOT be dropped).

Relay SHALL NOT inject ANSI/control sequences into ACP snapshot entries.

Legacy compatibility handoff behavior SHALL be:
- legacy flattened snapshot lines remain readable until first successful
  new-path `session/load`,
- first successful new-path `session/load` atomically replaces legacy baseline
  with canonical ACP snapshot entries,
- relay SHALL NOT keep mixed legacy/new composition after replacement.

#### Scenario: Use shared ACP worker as authoritative look snapshot writer

- **WHEN** relay serves ACP send and ACP look for one target session
- **THEN** both operations use one shared per-target ACP worker/client
- **AND** relay does not create a separate look-only ACP client for that target

#### Scenario: Replace baseline from load then append live updates in order

- **WHEN** relay receives `session/load` replay for target session
- **AND** later observes live `session/update` replay entries
- **THEN** relay replaces snapshot baseline from load replay
- **AND** appends live snapshot entries in ACP receive order
- **AND** preserves oldest->newest ordering in look responses

#### Scenario: Preserve unknown replay kinds via fallback entry

- **WHEN** relay observes an unknown or unsupported replay/update kind
- **THEN** relay emits fallback entry `kind="update"`
- **AND** relay does not silently drop the observed update

#### Scenario: Replace legacy flattened baseline on first successful structured load

- **WHEN** persisted snapshot contains legacy flattened lines
- **AND** first successful new-path `session/load` is ingested
- **THEN** relay replaces persisted snapshot with canonical ACP entry baseline
- **AND** subsequent look responses use canonical ACP entries only

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
- **AND** ACP additive freshness fields are omitted

#### Scenario: Return ACP look payload with structured entries

- **WHEN** look succeeds for ACP target
- **THEN** relay returns `snapshot_format="acp_entries_v1"`
- **AND** includes `snapshot_entries`
- **AND** includes required ACP additive fields `freshness` and
  `snapshot_source`

#### Scenario: Keep required ACP freshness fields when snapshot is empty

- **WHEN** ACP look succeeds with `snapshot_entries=[]`
- **THEN** relay still includes required `freshness` and `snapshot_source`

### Requirement: ACP Look Freshness Derivation

Relay SHALL evaluate ACP look freshness deterministically from shared worker
state and update recency.

MVP deterministic thresholds:

- `acp_look_prime_timeout_ms = 750`
- `acp_stream_stalled_after_ms = 5000`

Authoritative age source precedence SHALL be:
1. `last_acp_frame_observed_at_ms`
2. `last_snapshot_update_ms`
3. age unavailable (omit `snapshot_age_ms`)

Freshness predicate order SHALL be:
1. `worker_state=unavailable` => `freshness=stale`,
   `stale_reason_code=acp_worker_unavailable`.
2. snapshot empty:
   - prime timeout => `stale_reason_code=acp_snapshot_prime_timeout`
   - otherwise => `stale_reason_code=acp_worker_initializing`
3. snapshot non-empty:
   - `worker_state=busy` => `freshness=fresh` (MUST NOT emit
     `acp_stream_stalled`)
   - `worker_state=initializing` => `freshness=stale`,
     `stale_reason_code=acp_worker_initializing`
   - `worker_state=available` => `freshness=stale` with
     `stale_reason_code=acp_stream_stalled` only when stalled threshold is
     exceeded using authoritative age source precedence.

Relay SHALL treat machine freshness status as response-visible state for ACP
look.

Inscriptions/events MAY include the same freshness data as additive telemetry
but SHALL NOT be the sole machine carrier.

#### Scenario: Mark ACP look stale when worker is unavailable

- **WHEN** ACP look is served and shared worker state is unavailable
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_worker_unavailable`

#### Scenario: Keep ACP look fresh while worker is busy and snapshot exists

- **WHEN** ACP look is served with non-empty snapshot entries
- **AND** shared worker state is busy
- **THEN** relay returns `freshness=fresh`
- **AND** relay does not emit `stale_reason_code=acp_stream_stalled`

#### Scenario: Mark ACP look stale when available worker stream is stalled

- **WHEN** shared ACP worker is available
- **AND** no new ACP updates are observed for at least 5000ms using
  authoritative age-source precedence
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_stream_stalled`
