## 1. ACP module changes (first PR): in-memory replay accessor + buffer cap

- [x] 1.1 Add non-draining `read_replay_entries` accessor on `AcpStdioClient` (`src/acp/client.rs`). Existing draining `take_replay_entries` retained for the debug TUI.
- [x] 1.2 Enforce oldest-evict cap on the live replay buffer at 1000 entries; relocate or share `ACP_LOOK_SNAPSHOT_MAX_ENTRIES` constant.
- [x] 1.3 Update unit tests under `tests/integration/acp/` for the new accessor and cap discipline.

## 2. Relay module changes (second PR): persistence drop and rewiring

- [x] 2.1 Slim `PersistedAcpSessionState` (`src/relay/delivery/acp_state.rs`); bump `schema_version` to 2; drop `worker_state`, `snapshot_lines`, `snapshot_entries`, `last_acp_frame_observed_at_ms`, `last_snapshot_update_ms`.
- [x] 2.2 Make `state.json` reader fail-and-delete on unparseable; recreate via `session/new`.
- [x] 2.3 Remove snapshot persistence functions in `acp_state.rs` (`replace_acp_snapshot_entries_from_load` and peers).
- [x] 2.4 Introduce `worker_registry::set_state(target, state)` API (`src/relay/worker_registry.rs` or equivalent home).
- [x] 2.5 Replace all ~10 `persist_acp_worker_state` call sites in `src/relay/delivery/acp_delivery.rs` with `worker_registry::set_state` calls.
- [x] 2.6 Migrate `await_acp_worker_prime_for_look` (`src/relay/delivery/dispatch.rs:59-119`) to in-memory worker-registry query.
- [x] 2.7 Migrate `initialize_acp_target_for_startup` startup readiness-poll loop (`src/relay/delivery/dispatch.rs:178-219`) to in-memory worker-registry query.
- [x] 2.8 Migrate `handle_list` to `acp_session_ready_for_startup` (`src/relay/handlers.rs:288`) using in-memory worker-registry query.
- [x] 2.9 Migrate look handler (`src/relay/handlers.rs`) to read in-memory snapshot from worker via the new accessor.
- [x] 2.10 Update `tests/unit/relay.rs:331` (regression test from `8208b3e`).
- [x] 2.11 Update `tests/integration/acp/helpers.rs:500` (shared fixture helper); cascade flows through to `recovery.rs`, `worker_state.rs`, `lifecycle.rs`, `look.rs`, `serialization.rs`.
- [x] 2.12 Update `tests/integration/cli/look.rs`.
- [x] 2.13 Update `tests/integration/mcp/look.rs:144,193` for the new entry shape.

## 3. Joint validation

- [ ] 3.1 Cold-start: relay starts with no `state.json`; first `look` returns fresh-but-empty with `stale_reason_code=acp_worker_initializing` until prime completes; subsequent look returns full replay.
- [ ] 3.2 Warm restart: relay restarts with `state.json` carrying only `acp_session_id`; worker resumes via `session/load`; first `look` returns the upstream-replayed transcript without disk-cached intermediate state.
- [ ] 3.3 Worker drops: simulated worker unavailability; look returns `stale_reason_code=acp_worker_unavailable`.
- [ ] 3.4 Look concurrency: multiple concurrent `look` calls during prime do not race on in-memory snapshot reads.
- [ ] 3.5 List after restart: `handle_list` reports the bundle as `up`/`degraded` correctly via in-memory worker-registry queries; no `state.json` reads in the list path.
- [ ] 3.6 Surface parity: MCP/CLI/TUI look responses keep all freshness fields; entry shape change verified at `tests/integration/mcp/look.rs:144,193`.
- [ ] 3.7 `openspec validate refactor-acp-look-snapshot-to-in-memory --strict` passes.
