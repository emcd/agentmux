## ADDED Requirements

### Requirement: Relay reads adapter output through a poll look RPC

The system SHALL provide a relay→adapter per-call `look` RPC. The request SHALL carry a stable logical look-handle id that survives generation replacement (the handle rebinds), the adapter_generation per D2 (no epoch field), and optional `lines`, `offset`, and `prime_timeout_ms`. The relay SHALL pass only the timeout value as its look-surface policy; the bounded prime-wait SHALL run adapter-side. A zero timeout SHALL mean no wait.

#### Scenario: Prime-wait bounded by relay-supplied timeout

- **WHEN** the relay reads a still-initializing target with a nonzero prime timeout
- **THEN** the adapter waits up to that timeout for the first snapshot before returning a stale-tagged result

### Requirement: Look replies carry freshness and generation

Every look reply SHALL carry snapshot entries with full freshness metadata (`entries_total`, `returned_entries_count`, `freshness`, `snapshot_source`, optional `stale_reason_code`, optional `snapshot_age_ms`), the adapter_generation the snapshot was read from, and stale/discontinuity flags. The ACP profile SHALL map both replay-buffer and pane-capture reads onto snapshot_source live; synthetic marks constructed snapshots and MUST carry stale=true, never fresh. The relay SHALL NOT let a superseded generation's snapshot overwrite a newer one.

#### Scenario: Old generation reply loses to new snapshot

- **WHEN** a look reply from a replaced generation arrives after a newer generation's snapshot
- **THEN** the relay discards the stale reply and keeps the newer snapshot

### Requirement: Look schema stays streaming-compatible

The look schema SHALL preserve `offset` and entry counts so a future subscribe/stream attaches additively without breaking the poll shape. A prime-wait in progress SHALL NOT block cancel, fence, ack, or choice traffic. The relay's outer look deadline is the relay awaiting the adapter, not answering it: the relay SHALL retire timed-out correlations and SHALL distinguish no snapshot from stale cached output.

#### Scenario: Prime-wait does not block fence

- **WHEN** a fence begins while a look prime-wait is in progress
- **THEN** fence signaling and cessation observation proceed without waiting for the look to return

#### Scenario: Timed-out look retires its correlation

- **WHEN** the adapter exceeds the relay's outer look deadline
- **THEN** the relay retires the correlation and reports no snapshot — never a stale cached snapshot presented as fresh
