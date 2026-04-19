## 1. Spec Relock

- [ ] 1.1 Update `session-relay` startup requirements to lock two-phase startup
      evaluation and per-transport readiness predicates.
- [ ] 1.2 Lock bundle list state shape as non-breaking (`state=up|down`) with
      additive `startup_health` semantics for `state=up`.
- [ ] 1.3 Lock startup-failure visibility carriers (live event + persisted
      bounded history) and deterministic history ordering/retention rules.
- [ ] 1.4 Update `mcp-tool-surface` list contract with required startup
      health/failure fields and fallback semantics.
- [ ] 1.5 Update `cli-surface` list machine output/fallback contract with
      required startup health/failure fields.

## 2. Relay Runtime Implementation

- [ ] 2.1 Implement deterministic startup phase boundary:
      preflight then full per-session startup pass.
- [ ] 2.2 Implement bundle startup outcome evaluation:
      `state=up` when any session ready, `state=down` when zero ready.
- [ ] 2.3 Implement `startup_health=healthy|degraded` derivation for `state=up`.
- [ ] 2.4 Emit `relay.session_start_failed` for each failed startup attempt with
      canonical payload fields.
- [ ] 2.5 Persist bounded per-bundle startup-failure history (`max=256`,
      oldest-first eviction, monotonic `sequence`) and expose via list payload.
- [ ] 2.6 Keep process-level no-selector host startup semantics unchanged.

## 3. Adapter and Output Surfaces

- [ ] 3.1 Update MCP list serialization/tests for required startup health and
      startup-failure fields.
- [ ] 3.2 Update CLI list machine output tests for required startup health and
      startup-failure fields.
- [ ] 3.3 Lock deterministic synthesized fallback behavior for startup failure
      fields when relay is unreachable.

## 4. Validation

- [ ] 4.1 Run `openspec validate update-bundle-startup-health-and-failure-visibility-mvp --strict`.
- [ ] 4.2 Run targeted relay lifecycle/list tests.
- [ ] 4.3 Run targeted CLI and MCP list tests.
