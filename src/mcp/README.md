# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list`
  - `look`
  - `send`
- Validate MCP request payloads and map validation errors to structured MCP
  errors.
- Forward valid requests to relay over the bundle Unix socket.
- Emit MCP inscriptions for request, success, and error events.

## Data Flow

1. MCP client calls `list`, `look`, or `send`.
2. `src/mcp/mod.rs` validates parameters.
3. Request is translated to relay contract:
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
