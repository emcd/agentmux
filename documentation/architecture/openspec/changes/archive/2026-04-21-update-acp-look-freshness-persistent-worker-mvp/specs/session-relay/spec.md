## MODIFIED Requirements

### Requirement: ACP Look Snapshot Contract

Relay look SHALL support ACP-backed target sessions using relay-managed
snapshot state populated from a shared per-target ACP worker/client.

For ACP targets, relay SHALL:
- use the same shared per-target ACP worker/client used by ACP send lifecycle
  and prompt execution,
- ingest non-empty text lines from ACP `session/update` payloads into a live
  worker-owned snapshot buffer,
- retain at most 1000 lines per session,
- evict oldest lines first when retention exceeds 1000,
- return look results ordered oldest -> newest,
- return tail lines based on requested `lines`,
- avoid spawning a second ACP client for steady-state look requests.

#### Scenario: Use shared ACP worker as authoritative look snapshot writer

- **WHEN** relay serves ACP send and ACP look for one target session
- **THEN** both operations use one shared per-target ACP worker/client
- **AND** relay does not create a separate look-only ACP client for that target

#### Scenario: Enforce bounded ACP look retention and oldest-first eviction

- **WHEN** retained ACP snapshot lines for one target exceed 1000
- **THEN** relay evicts oldest lines first
- **AND** subsequent look requests return at most 1000 retained lines

#### Scenario: Preserve existing tmux look behavior unchanged

- **WHEN** requester invokes relay `look` for a target session backed by tmux
  transport
- **THEN** relay executes canonical tmux look capture behavior unchanged

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends and ACP look
snapshot ingestion.

Worker model SHALL be:

- one worker per target session
- serialized request queue per worker
- fixed MVP queue bound `max_pending = 64`
- initialized during bundle startup/session startup pass for hosted bundles
- anchored by relay runtime context (relay socket/runtime directory), not tmux
  transport semantics
- never lazily created by ACP send/look request handlers

Backpressure contract:

- enqueue beyond bound SHALL fail with `runtime_acp_queue_full`

Disconnect/restart contract:

- disconnect before phase-1 acknowledgment =>
  `runtime_acp_connection_closed`
- disconnect after phase-1 acknowledgment SHALL keep response immutable and
  transition worker to `unavailable` for recovery

Restart sequence SHALL be:

1. spawn ACP process
2. initialize
3. select lifecycle (`session/load` when identity exists, else `session/new`)
4. prompt

Failure taxonomy SHALL include:

- `runtime_acp_initialize_failed`
- `runtime_acp_session_load_failed`
- `runtime_acp_session_new_failed`
- `runtime_acp_prompt_failed`
- `acp_turn_timeout`
- `runtime_acp_worker_unavailable`

#### Scenario: Keep one authoritative worker for ACP send and look ingestion

- **WHEN** relay handles ACP send requests and ACP look reads for one target
- **THEN** lifecycle/reconnect ownership remains with one shared worker
- **AND** relay avoids dual ACP worker/client ownership for that target

#### Scenario: Start ACP worker during startup pass without lazy send/look bootstrap

- **WHEN** relay runs startup pass for a hosted bundle with ACP targets
- **THEN** relay initializes one ACP worker per configured ACP target
- **AND** ACP send/look request handlers do not lazily create ACP workers

#### Scenario: Return deterministic unavailable outcome when ACP worker is absent

- **WHEN** ACP send or ACP look is requested for a target whose ACP worker is
  unavailable
- **THEN** relay does not spawn a request-scoped ACP client
- **AND** send returns failure with `runtime_acp_worker_unavailable`
- **AND** look returns stale metadata with
  `stale_reason_code=acp_worker_unavailable`

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

For ACP targets, successful relay look responses SHALL additionally include:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when unavailable)

`snapshot_lines` ordering SHALL be oldest-to-newest.

ACP stale reason vocabulary SHALL be fixed in MVP:

- `acp_worker_initializing`
- `acp_worker_unavailable`
- `acp_snapshot_prime_timeout`
- `acp_stream_stalled`

#### Scenario: Return canonical look payload for tmux target

- **WHEN** look succeeds for tmux target
- **THEN** relay returns canonical look response fields
- **AND** `snapshot_lines` contains ordered snapshot lines from oldest to
  newest
- **AND** ACP additive freshness fields are omitted

#### Scenario: Return ACP look payload with required freshness fields

- **WHEN** look succeeds for ACP target
- **THEN** relay returns canonical look response fields
- **AND** includes required ACP additive fields `freshness` and
  `snapshot_source`

#### Scenario: Return stale-success metadata on ACP prime timeout

- **WHEN** first ACP look for a target cannot complete worker init/prime within
  750ms
- **THEN** relay still returns successful look payload
- **AND** `freshness=stale`
- **AND** `snapshot_source=none`
- **AND** `stale_reason_code=acp_snapshot_prime_timeout`

#### Scenario: Keep required ACP freshness fields when snapshot is empty

- **WHEN** ACP look succeeds with `snapshot_lines=[]`
- **THEN** relay still includes required `freshness` and `snapshot_source`

## ADDED Requirements

### Requirement: ACP Look Freshness Derivation

Relay SHALL evaluate ACP look freshness deterministically from shared worker
state and update recency.

MVP deterministic thresholds:

- `acp_look_prime_timeout_ms = 750`
- `acp_stream_stalled_after_ms = 5000`

Relay SHALL treat machine freshness status as response-visible state for ACP
look.

Inscriptions/events MAY include the same freshness data as additive telemetry
but SHALL NOT be the sole machine carrier.

#### Scenario: Mark ACP look stale when worker is initializing

- **WHEN** ACP look is served while shared worker is initializing and no ready
  snapshot is available
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_worker_initializing`

#### Scenario: Mark ACP look stale when stream is stalled

- **WHEN** shared ACP worker is connected
- **AND** no new ACP updates are observed for at least 5000ms
- **AND** no prompt turn is in flight
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_stream_stalled`
