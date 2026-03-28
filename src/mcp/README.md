# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list`
  - `look`
  - `send`
- Validate MCP request payloads.
- Forward valid requests to relay over the bundle Unix socket.
- Preserve relay error taxonomy/details when relay returns structured errors.
- Emit MCP inscriptions for request, success, and error events.

## Data Flow

1. MCP client calls `list`, `look`, or `send`.
2. `src/mcp/mod.rs` validates parameter shape and transport-compatible options.
3. Request is forwarded as relay contract over `RelayStreamSession`:
   - `list` -> `RelayRequest::List`
   - `look` -> `RelayRequest::Look`
   - `send` -> `RelayRequest::Chat`
4. Relay response is mapped back to MCP JSON payload.

## Key Types

- `McpConfiguration`
  - startup configuration for bundle paths and sender session identity.
- `McpServer`
  - tool router + handlers.
- `SendParams`
  - MCP `send` request schema, including `delivery_mode` and optional
  transport-scoped timeout overrides (`quiescence_timeout_ms`,
  `acp_turn_timeout_ms`).

## Validation and Error Policy

- MCP rejects invalid request shapes before relay submission (for example empty
  targets or conflicting timeout fields).
- MCP does not perform shadow authorization checks.
- Relay `authorization_forbidden` and other relay codes are passed through as
  MCP errors with relay details.

## Event Handling Note

- Relay may return stream events alongside direct responses.
- Current MCP behavior logs these events via inscriptions and ignores them at
  tool-response level.
