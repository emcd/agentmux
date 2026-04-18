# Change: Relock MCP list.sessions to list meta-tool

## Why

In this environment, adding top-level MCP tools creates recurring operational
cost:

- per-tool harness permission configuration updates,
- frequent harness restarts to refresh tool inventory,
- larger top-level tool schema footprint in prompt context.

Because this project is pre-stable, we can relock now without compatibility
shims.

## What Changes

- Relock MCP top-level tool name from `list.sessions` to `list`.
- Use a meta-tool envelope for list operations:
  - `command` (required; MVP requires `"sessions"`),
  - `args` (optional object; MVP supports `bundle_name` and `all`).
- Preserve existing selector behavior:
  - `bundle_name` and `all=true` remain mutually exclusive,
  - associated/home bundle remains the default when selector omitted,
  - all-mode fanout behavior remains unchanged.
- Preserve canonical response payload shapes unchanged.
- Keep relay and CLI contracts unchanged in this slice.

## Impact

- Affected specs:
  - `mcp-tool-surface`
- Affected code:
  - `src/mcp/mod.rs`
  - `tests/integration/mcp/list.rs`
  - `tests/integration/runtime_bootstrap.rs`
  - `src/mcp/README.md`

## Breaking Changes (pre-stable intentional)

- MCP clients must call `list` instead of `list.sessions`.
- MCP clients must provide `command="sessions"` for list operations.
- No alias/compatibility shim is provided in this relock.
