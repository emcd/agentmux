## 1. Spec Relock

- [ ] 1.1 Update `session-relay` ACP look requirements to shared-worker live buffer ownership.
- [ ] 1.2 Lock deterministic first-look cold-start behavior with fixed prime timeout (`750ms`) and stale-success metadata semantics.
- [ ] 1.3 Lock freshness derivation/staleness threshold (`5000ms`) and stale reason vocabulary.
- [ ] 1.4 Update `mcp-tool-surface` look response contract to preserve additive ACP freshness fields unchanged.
- [ ] 1.5 Update `cli-surface` ACP look success surface to preserve additive ACP freshness fields in machine output.

## 2. Relay Runtime Implementation

- [ ] 2.1 Ensure single shared per-target ACP worker/client is authoritative for both send and look ingestion.
- [ ] 2.2 Route ACP look reads to worker-owned live snapshot state (remove steady-state one-shot refresh dependency).
- [ ] 2.3 Implement deterministic cold-start prime behavior (`750ms`) with required ACP freshness fields.
- [ ] 2.4 Implement deterministic stale reason mapping (`acp_worker_initializing`, `acp_worker_unavailable`, `acp_snapshot_prime_timeout`, `acp_stream_stalled`).
- [ ] 2.5 Keep event/inscription freshness carriers additive only (not required for machine correctness).

## 3. Adapter and Serialization

- [ ] 3.1 Update MCP look serialization/passthrough tests for additive ACP freshness fields.
- [ ] 3.2 Update CLI look machine output tests for additive ACP freshness fields.
- [ ] 3.3 Lock requiredness behavior for ACP look fields when `snapshot_lines=[]` and when `snapshot_source=none`.

## 4. Validation

- [ ] 4.1 Run `openspec validate update-acp-look-freshness-persistent-worker-mvp --strict`.
- [ ] 4.2 Run targeted relay ACP look tests.
- [ ] 4.3 Run targeted MCP/CLI look passthrough tests.
