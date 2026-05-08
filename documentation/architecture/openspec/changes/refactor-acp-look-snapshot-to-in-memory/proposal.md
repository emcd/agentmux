# Change: Refactor ACP Look Snapshot To In-Memory (Drop Persistence)

## Why

Relay currently persists a structured snapshot of ACP conversation state
(`snapshot_entries`, `snapshot_lines`, freshness timestamps) to `state.json`
on every replay update so that `look` can render the transcript across relay
restarts. This duplicates state owned by the upstream ACP server, which is
already authoritative and provides full replay via `session/load` on
reconnect. Two concrete costs:

- Schema migration burden. Every change to `AcpSnapshotEntry` becomes a
  serialization-contract change. Recent symptom: parse failures on
  `state.json` written before the `Invocation` coalescing rework added
  required `call_id`/`status` fields, surfacing in the TUI as "failed to
  await ACP worker prime for look: relay returned internal error" and
  inscribing repeated `internal_unexpected_failure` events at bundle
  startup.
- Two sources of truth. The relay's in-memory snapshot (held by the live
  ACP client) and the on-disk snapshot can drift on partial writes,
  crashes, or aborted sessions.

## What Changes

- **BREAKING:** Drop persistence of ACP look snapshot data in
  `state.json`. Persisted shape reduces to `schema_version` (bump to 2)
  and `acp_session_id` only.
- **BREAKING:** Reconcile snapshot entry vocabulary with the coalesced
  shape in current code: `kind = "invocation"` carries `status` and
  optional `result` inline; separate `kind = "result"` entry is removed
  (already absent from current code; spec catches up).
- Make eager pre-warm via `initialize_acp_target_for_startup`
  (`src/relay/delivery/dispatch.rs:121`, called from
  `src/relay/lifecycle.rs:190`) the sole rehydration path. First `look`
  after relay restart returns fresh-but-empty with
  `stale_reason_code=acp_worker_initializing` until prime completes;
  existing freshness vocabulary covers all cases that previously had a
  disk fallback.
- Add non-draining `read_replay_entries` accessor on `AcpStdioClient`
  (existing draining `take_replay_entries` retained for the debug TUI).
  Live replay buffer enforces oldest-evict cap at 1000 entries, mirroring
  current `ACP_LOOK_SNAPSHOT_MAX_ENTRIES` from the persisted path.
- Migrate three relay disk-read sites to in-memory worker-registry
  queries: `await_acp_worker_prime_for_look`,
  `initialize_acp_target_for_startup` startup poll loop, and
  `handle_list` to `acp_session_ready_for_startup`.
- Introduce `worker_registry::set_state(target, state)` API to replace
  ~10 open-coded `persist_acp_worker_state` call sites in
  `acp_delivery.rs`. Worker readiness state is process-local in-memory
  only.
- Migration posture: fail+recreate. On first relay startup after this
  change, an unparseable `state.json` is logged + deleted and a new
  upstream session is created via `session/new`. Cost is one extra
  `session/new` per ACP target at the upgrade moment; acceptable
  pre-MVP.

## Impact

- Affected specs:
  - `session-relay` — modify ACP Look Snapshot Contract (drop
    persistence semantics, reconcile entry vocabulary to coalesced
    shape, drop legacy-migration handoff).
  - `acp-client` — add Non-Draining Replay Accessor and Replay Buffer
    Cap requirements.
- Affected code:
  - `src/relay/delivery/acp_state.rs` — slim `PersistedAcpSessionState`
  - `src/relay/delivery/acp_delivery.rs` — remove all snapshot persistence calls
  - `src/relay/delivery/dispatch.rs` — both poll loops to in-memory
  - `src/relay/handlers.rs` — look path + list path to in-memory
  - `src/relay/worker_registry.rs` (new home) — `set_state` API
  - `src/acp/client.rs` — non-draining accessor, buffer cap-and-evict
  - Tests: `tests/unit/relay.rs`, `tests/integration/acp/helpers.rs`,
    `tests/integration/cli/look.rs`, `tests/integration/mcp/look.rs`
- Surface confirmations recorded in `coordination/acp/3` (TUI, MCP,
  relay second-look) remain accurate as discovery findings; `call_id`
  retention on the in-memory invocation entry means those confirmations
  are no longer gating. Implementation owner: Backend Engineer for both
  module-scoped PRs.

## Sequencing

Backend Engineer owns implementation. Work splits into two sequential
PRs to avoid a half-state where the persisted struct is slim but
`acp_delivery.rs` still tries to write the dropped fields.

1. First PR (ACP module changes; small, isolated): non-draining
   `read_replay_entries` accessor on `AcpStdioClient`; buffer
   cap-and-evict-oldest at 1000.
2. Second PR (relay module changes; wholesale): slim
   `PersistedAcpSessionState`; rewire list+look paths; swap both
   `dispatch.rs` poll loops; remove persistence calls; introduce
   `worker_registry::set_state` API; update tests/fixtures.
3. Joint validation matrix per `tasks.md` section 3.
