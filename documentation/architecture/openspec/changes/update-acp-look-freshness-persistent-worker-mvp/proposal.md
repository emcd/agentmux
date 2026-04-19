# Change: Relock ACP Look Freshness to Shared Persistent Worker

## Why
`issues/acp/5` showed relay `look` returning stale ACP snapshots versus live ACP pane state. The current one-shot refresh path is request-time bounded and can miss late updates. We need deterministic freshness behavior aligned with persistent ACP client patterns.

## What Changes
- Relock relay ACP look freshness to one shared persistent ACP worker/client per target session.
- Remove one-shot per-request ACP refresh from steady-state look path.
- Define deterministic first-look cold-start behavior with fixed bounded prime timeout.
- Lock canonical MVP behavior as success-with-explicit-stale-metadata (no silent stale-success, no fail-fast error on prime timeout).
- Add additive ACP look freshness fields across relay/MCP/CLI surfaces while keeping canonical look fields unchanged.
- Keep event/inscription freshness carriers additive only.

## Impact
- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code:
  - relay ACP worker lifecycle/state ownership paths
  - relay look ACP read path
  - MCP and CLI look passthrough serialization/tests
