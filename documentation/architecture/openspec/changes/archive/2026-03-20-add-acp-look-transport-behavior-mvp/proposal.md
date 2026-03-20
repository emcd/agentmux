# Change: Add ACP look bounded snapshot behavior for MVP

## Why

ACP send runtime behavior now executes prompt turns and receives
`session/update` stream events. Keeping ACP `look` unsupported leaves operators
and agents blind during ACP validation and smoke testing.

## What Changes

- Add relay requirement for ACP look snapshot ingestion from `session/update`
  text fields during prompt turns.
- Add deterministic bounded retention contract for ACP look snapshots:
  - max retained lines: 1000
  - eviction policy: oldest-first
  - look ordering: oldest -> newest
- Add ACP-target look retrieval contract:
  - return tail slice based on requested `lines`
  - return empty `snapshot_lines` when no ACP snapshot exists
- Require MCP and CLI adapters to preserve the relay-authored ACP look success
  payload shape unchanged.
- Preserve existing tmux look behavior unchanged.

## Non-Goals

- Adding ACP HTTP look transport behavior.
- Changing `look` request parameters.
- Introducing cross-bundle look support.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Expected follow-up implementation touchpoints:
  - relay ACP update ingestion and snapshot persistence
  - relay ACP look retrieval path
  - MCP/CLI ACP look payload passthrough tests
