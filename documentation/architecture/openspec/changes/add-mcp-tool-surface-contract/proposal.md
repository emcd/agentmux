# Change: Add MCP tool surface contract for session relay

## Why

The relay MVP defines behavior, but implementers and LLM clients still need a
stable MCP contract for tool names, request schemas, response schemas, and
error codes. A dedicated tool-surface spec reduces integration ambiguity and
makes multi-agent workflows predictable.

## What Changes

- Add a new `mcp-tool-surface` capability for `tmuxmux`.
- Define an MVP-minimal MCP tool set with:
  - `list` for discovering potential recipient sessions.
  - `chat` for message delivery.
- Assume bundles are configured manually outside MCP for MVP.
- Define `chat` target selection contract as explicit `targets[]` or
  `broadcast=true`.
- Define sender identity inference from MCP server session association.
- Define optional human-friendly recipient and sender display names distinct
  from routing session names.
- Define synchronous delivery result schema with per-target outcomes.
- Define stable machine-readable error codes and error object shape.
- Define versioning and compatibility expectations for tool schemas.

## Impact

- Affected specs: `mcp-tool-surface` (new capability).
- Related specs: `session-relay` from `add-mcp-session-relay-mvp`.
- Affected code:
  - MCP server tool registration and handlers.
  - Request validation layer, including target-mode validation.
  - Response serialization and schema versioning.
