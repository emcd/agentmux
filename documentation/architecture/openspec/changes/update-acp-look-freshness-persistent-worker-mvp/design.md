## Context

Relay ACP look currently depends on retained snapshot state fed by prompt-turn update windows. This can lag the live ACP pane. The human direction is to use a persistent ACP client architecture (same model as direct ACP workflows), not request-scoped client refresh.

## Goals

- Make ACP look freshness deterministic and relay-owned.
- Prevent dual ACP clients per target (send worker versus look worker).
- Preserve existing canonical look payload fields and add freshness metadata as additive fields.
- Lock one canonical MVP degraded behavior for prime-timeout/unavailable cases.

## Non-Goals

- No cross-bundle look support changes.
- No change to tmux look semantics.
- No broad authorization redesign.

## Decisions

- Decision: one shared authoritative per-target ACP worker/client is used for both send lifecycle and look snapshot ingestion.
- Decision: ACP worker lifecycle is startup-owned for hosted bundles.
  - ACP workers are initialized during bundle startup/session startup pass.
  - ACP workers remain authoritative while bundle state is `up`.
  - ACP send/look paths SHALL NOT lazily create workers at request time.
- Decision: runtime anchoring for ACP worker keys/state uses relay runtime
  context (relay socket/runtime directory), not tmux transport semantics.
- Decision: MVP removes `persisted_fallback` snapshot source vocabulary for look freshness; source is `live_buffer` or `none`.
- Decision: first ACP look cold-start uses fixed prime timeout `750ms`.
  - prime success: success payload with `freshness=fresh`.
  - prime timeout/unavailable/initializing/stalled: success payload with `freshness=stale` and deterministic `stale_reason_code`.
- Decision: stale-success remains canonical in MVP (no fail-fast error on prime timeout).
- Decision: additive freshness fields for ACP look response are:
  - required: `freshness`, `snapshot_source`
  - conditional required: `stale_reason_code` when `freshness=stale`
  - optional: `snapshot_age_ms`
- Decision: stale reason vocabulary in MVP:
  - `acp_worker_initializing`
  - `acp_worker_unavailable`
  - `acp_snapshot_prime_timeout`
  - `acp_stream_stalled`
- Decision: stream stalled threshold fixed to `5000ms` in MVP.
- Decision: machine freshness status must be visible in canonical look response; inscriptions/events are additive only.

## Risks / Trade-offs

- Shared worker ownership reduces freshness drift but increases coupling between send and look paths.
- Startup-owned workers increase deterministic behavior and simplify look/send
  semantics, but can increase startup failure volume and startup-time process
  fanout for bundles with ACP sessions.
- Fixed thresholds improve determinism and testability but may need post-MVP tuning.
- Success-with-stale metadata avoids hard failures but requires operator/adapter awareness.

## Migration Plan

1. Update specs (`session-relay`, `mcp-tool-surface`, `cli-surface`) with relocked behavior.
2. Relock worker lifecycle to startup-owned initialization and remove
   request-time worker bootstrap for ACP send/look.
3. Move ACP look path to shared worker-owned live snapshot buffer.
4. Remove steady-state one-shot look refresh path.
5. Add/update tests for startup-owned worker behavior, stale signaling,
   passthrough parity, and snapshot requiredness.
