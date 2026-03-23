# Change: ACP Persistent Transport and Delivery Semantics (MVP)

## Why

ACP send behavior currently reuses tmux-oriented timeout naming and launches a
fresh ACP stdio subprocess per delivery. For high-latency models, this creates
false timeouts, abrupt transport teardown on stdin close, and unclear sync
completion semantics.

## What Changes

- Split timeout semantics between tmux and ACP:
  - request-level `acp_turn_timeout_ms` for ACP turn wait bounds
  - coder-level `[coders.acp] turn-timeout-ms` default
  - reject transport-incompatible timeout fields with canonical validation codes
- Lock ACP sync send as a two-phase contract:
  - sync success on first ACP activity (`session/update` or prompt result)
  - early-success marker `details.delivery_phase = "accepted_in_progress"`
  - terminal completion used for relay-internal worker readiness only in MVP
- Add persistent ACP worker lifecycle contract:
  - one worker per target session
  - serialized queue, fixed MVP bound `max_pending = 64`
  - canonical backpressure and disconnect/restart failure taxonomy
- Lock MVP permission-request readiness behavior:
  - treat ACP `session/request_permission` as in-progress activity
  - keep ACP worker non-ready (`busy`) until terminal completion
  - defer policy-driven permission allow/deny mapping to a follow-up delta

## Non-Goals (MVP)

- ACP HTTP transport implementation.
- Full ACP look redesign.
- Full ACP permission decisioning and deny/timeout taxonomy.
- Backward-compat timeout aliases or silent field translation.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code (expected):
  - relay ACP delivery runtime and worker lifecycle
  - send request validation/parsing in relay, CLI, and MCP adapter
  - relay ACP worker availability state transitions after terminal stop-reason
