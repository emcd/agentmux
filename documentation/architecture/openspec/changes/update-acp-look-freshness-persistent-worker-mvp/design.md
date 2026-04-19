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
- Decision: worker startup remains lazy (first ACP send/look), not eager at relay host startup.
  - Rationale: avoids coupling relay host startup success to ACP runtime availability,
    preserves process-only/no-autostart host behavior, and prevents unnecessary ACP
    process fanout for sessions never targeted.
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
- Fixed thresholds improve determinism and testability but may need post-MVP tuning.
- Success-with-stale metadata avoids hard failures but requires operator/adapter awareness.

## Migration Plan

1. Update specs (`session-relay`, `mcp-tool-surface`, `cli-surface`) with relocked behavior.
2. Move ACP look path to shared worker-owned live snapshot buffer.
3. Remove steady-state one-shot look refresh path.
4. Add/update tests for cold-start, stale signaling, passthrough parity, and snapshot requiredness.
