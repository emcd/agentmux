## 1. Spec Relock

- [x] 1.1 Update `session-relay` ACP look requirements to shared-worker live buffer ownership.
- [x] 1.2 Relock ACP worker lifecycle to startup-owned initialization (no lazy send/look worker bootstrap).
- [x] 1.3 Lock runtime anchoring semantics to relay runtime context (not tmux transport naming).
- [x] 1.4 Lock deterministic first-look cold-start behavior with fixed prime timeout (`750ms`) and stale-success metadata semantics.
- [x] 1.5 Lock freshness derivation/staleness threshold (`5000ms`) and stale reason vocabulary.
- [x] 1.6 Update `mcp-tool-surface` look response contract to preserve additive ACP freshness fields unchanged.
- [x] 1.7 Update `cli-surface` ACP look success surface to preserve additive ACP freshness fields in machine output.

## 2. Relay Runtime Implementation

- [x] 2.1 Ensure single shared per-target ACP worker/client is authoritative for both send and look ingestion.
- [x] 2.2 Initialize ACP workers during startup pass for hosted bundles; do not create them lazily in ACP send/look handlers.
- [x] 2.3 Route ACP look reads to worker-owned live snapshot state (remove steady-state one-shot refresh dependency).
- [x] 2.4 Implement deterministic cold-start prime behavior (`750ms`) with required ACP freshness fields.
- [x] 2.5 Implement deterministic stale reason mapping (`acp_worker_initializing`, `acp_worker_unavailable`, `acp_snapshot_prime_timeout`, `acp_stream_stalled`).
- [x] 2.6 Keep event/inscription freshness carriers additive only (not required for machine correctness).
- [x] 2.7 Replace ACP runtime key/state anchoring that references tmux socket naming with relay runtime context anchoring, including rename cleanup for bridge-era ACP helpers (`bootstrap_acp_worker_runtime`, `await_acp_worker_prime_for_look`).

## 3. Adapter and Serialization

- [x] 3.1 Update MCP look serialization/passthrough tests for additive ACP freshness fields.
- [x] 3.2 Update CLI look machine output tests for additive ACP freshness fields.
- [x] 3.3 Lock requiredness behavior for ACP look fields when `snapshot_lines=[]` and when `snapshot_source=none`.
- [x] 3.4 Add regression coverage that ACP send/look do not lazily bootstrap unavailable workers.

## 4. Validation

- [x] 4.1 Run `openspec validate update-acp-look-freshness-persistent-worker-mvp --strict`.
- [x] 4.2 Run targeted relay ACP look tests.
- [x] 4.3 Run targeted MCP/CLI look passthrough tests.
