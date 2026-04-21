## Context

Startup should be resilient at bundle scope: if at least one session starts,
bundle hosting remains available while failures are surfaced deterministically.
Current contracts do not fully lock startup health semantics or late-reader
failure visibility across relay/CLI/MCP list surfaces.

## Goals

- Lock deterministic startup evaluation and readiness predicates.
- Preserve existing process-level host startup semantics.
- Preserve non-breaking list bundle shape while adding degraded-health signal.
- Provide mandatory machine-visible startup failure evidence for live readers
  and late readers.

## Non-Goals

- No process-level host exit-policy relock.
- No retry manager/auto-heal orchestration.
- No new dedicated catch-up query API in MVP.

## Decisions

- Decision: startup uses two phases:
  1. bundle preflight
  2. full per-session startup pass (attempt all configured sessions) when
     preflight succeeds.
- Decision: preflight failure returns `state=down` with
  `state_reason_code=runtime_startup_failed` and no per-session startup pass.
- Decision: state shape remains `state=up|down`.
- Decision: degraded condition is additive:
  - `startup_health` required when `state=up`
  - values: `healthy|degraded`.
- Decision: startup failure visibility uses two carriers:
  - live event/inscription `relay.session_start_failed`
  - persisted bounded history surfaced by list payload fields
    `startup_failure_count` and `recent_startup_failures`.
- Decision: persisted history retention is fixed at 256 records per bundle,
  oldest-first eviction, oldest->newest read order, monotonic per-bundle
  `sequence`.
- Decision: `runtime_listener_bind_failed` remains process-level host startup
  summary/failure semantics and is excluded from bundle list-state precedence.

## Risks / Trade-offs

- Additive health/failure fields increase payload complexity but avoid breaking
  existing `state=up|down` consumers.
- Persisted history improves observability but introduces state-lifecycle rules
  that must remain deterministic across restarts.

## Migration Plan

1. Relock `session-relay` startup phases, readiness predicates, failure
   visibility, and list payload field requirements.
2. Relock MCP and CLI list payload passthrough/output contracts for new fields.
3. Implement relay startup-failure emission + persistence and list serialization
   updates.
4. Add integration tests for degraded startup, down-state precedence, and list
   field behavior in normal and fallback paths.
