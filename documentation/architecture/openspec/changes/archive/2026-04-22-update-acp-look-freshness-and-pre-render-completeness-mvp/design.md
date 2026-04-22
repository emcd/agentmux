## Context

Recent field observations and cross-lane review found two ACP look gaps:

1. freshness signaling is not always intuitive around busy/idle transitions,
2. ACP look payload content can lose structure compared to the
   `agentmux-acp` debug TUI replay rendering.

The goal is to relock deterministic freshness behavior and move ACP look payload
to typed structured entries while keeping tmux payload shape unchanged.

## Goals

- Make ACP freshness predicates fully deterministic and ordered.
- Use one shared conversion path for ACP replay entry -> structured snapshot
  entry conversion.
- Preserve typed conversation structure in wire payload for ACP look.
- Keep tmux look payload shape unchanged.
- Avoid dual ingestion/write paths that can reorder or duplicate history.

## Non-Goals

- No tmux look behavior changes.
- No cross-bundle look support changes.
- No ANSI/control escape sequences in wire payload.
- No schema unification forcing tmux to adopt ACP structured entries.

## Decisions

- Decision: shared ACP snapshot conversion is canonical at `src/acp/render.rs`,
  reused by relay and `agentmux-acp`.
- Decision: look response uses discriminator field:
  - `snapshot_format = "lines"` for tmux targets with `snapshot_lines`,
  - `snapshot_format = "acp_entries_v1"` for ACP targets with
    `snapshot_entries`.
- Decision: ACP structured entry vocabulary is fixed in MVP:
  - `user` with `lines: string[]`
  - `agent` with `lines: string[]`
  - `cognition` with `lines: string[]`
  - `invocation` with `invocation: object` (pass-through)
  - `result` with `result: object` (pass-through)
  - `update` fallback with `update_kind: string`, `lines: string[]`
- Decision: unknown/unsupported replay/update kinds map to fallback
  `kind="update"` and MUST NOT be dropped.
- Decision: wire payload remains ANSI/control-sequence free; clients MAY apply
  ANSI/SGR when rendering locally.
- Decision: one authoritative relay ingestion/write path handles:
  - `session/load` replay via replace baseline
  - live `session/update` replay via append in receive order
- Decision: MVP keeps no dedupe; observed source order is authoritative.
- Decision: retention remains bounded to existing ACP snapshot cap with
  oldest-first eviction.
- Decision: freshness order/precedence is fixed:
  1. worker unavailable => stale (`acp_worker_unavailable`)
  2. empty snapshot => stale (`acp_snapshot_prime_timeout` or
     `acp_worker_initializing`)
  3. non-empty snapshot:
     - busy => fresh (never `acp_stream_stalled`)
     - available => stale only when threshold exceeded
- Decision: age source precedence is fixed:
  - `last_acp_frame_observed_at_ms`
  - then `last_snapshot_update_ms`
  - else omitted
- Decision: no separate offline migration is required for legacy flattened ACP
  snapshot state.
  - compatibility handoff is replace-on-first-successful-structured-load:
    legacy flattened lines are ignored for look responses until first successful
    new-path `session/load` atomically replaces legacy baseline with canonical
    structured entries.

## Risks / Trade-offs

- No-dedupe MVP keeps source fidelity but can preserve repeated content in some
  ACP server patterns.
- Structured ACP entry payload is a pre-MVP breaking change and requires
  adapter/test updates across CLI/MCP.
- Preserving tmux line payload while ACP becomes structured introduces a
  response union that clients must branch on via `snapshot_format`.

## Migration Plan

1. Land OpenSpec deltas for `session-relay`, `mcp-tool-surface`,
   `cli-surface`.
2. Implement shared ACP conversion and route relay ACP snapshot construction
   through it.
3. Relock freshness derivation to ordered predicate table and source precedence.
4. Add replace-on-first-successful-structured-load compatibility handoff for
   legacy flattened ACP snapshot state.
5. Add ordering, compatibility handoff, and freshness transition regression
   tests.
