# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list` (MVP requires `command="sessions"`)
  - `help`
  - `look`
  - `send`
- Preserve canonical relay `look` success payloads without adapter reshaping:
  - tmux: `snapshot_format="lines"` + `snapshot_lines`
  - ACP: `snapshot_format="acp_entries_v1"` + `snapshot_entries` (+ freshness fields)
- Validate MCP request payloads.
- Forward valid requests to relay over the bundle Unix socket.
- Preserve relay error taxonomy/details when relay returns structured errors.
- Emit MCP inscriptions for request, success, and error events.

## Data Flow

1. MCP client calls `list`, `look`, or `send`.
2. MCP client can call `help` to discover tool/command schemas.
3. `src/mcp/mod.rs` validates parameter shape and transport-compatible options.
4. Request is forwarded as relay contract:
   - `list` (`command="sessions"`) -> one-shot `request_relay` probes (`RelayRequest::List`)
   - `look` -> `RelayStreamSession` (`RelayRequest::Look`)
   - `send` -> `RelayStreamSession` (`RelayRequest::Chat`)
5. For `all=true`, MCP performs adapter fanout across bundle relays in
   deterministic lexicographic order.
6. Relay response is mapped back to MCP JSON payload.

## Key Types

- `McpConfiguration`
  - startup configuration for runtime roots, bundle paths, and sender session
    identity.
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
