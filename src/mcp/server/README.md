# MCP Server

This directory holds the MCP stdio server implementation: the public
boot entrypoint, the shared state type, and the per-tool
`#[tool_router]` handler composition.

It is the implementation of the surface described in the parent
`src/mcp/README.md`. The cross-cutting helpers — `errors.rs`,
`params.rs`, `validation.rs`, `help.rs` — live as siblings under
`src/mcp/`, not in this directory, so they can be referenced from
both the per-tool handlers and the validation entrypoints without
creating a cycle through the server module.

## Directory layout

- `mod.rs`
  - Operator-hub: only `mod` declarations and re-exports. Re-exports
    `McpConfiguration` and `run` from `service`; `McpServer` is
    re-exported with `pub(crate)` visibility for use by the per-tool
    handlers in `handlers/`.
- `service.rs`
  - `McpConfiguration` (startup configuration) and `McpState`
    (runtime state held behind `Arc`).
  - `McpServer` struct (`pub(crate) state: Arc<McpState>` plus
    `tool_router: ToolRouter<Self>`).
  - `McpServer::new()` composes the per-tool routers in registration
    order via the `Add` impl on `ToolRouter`: list, help, send,
    look, raww, choose, updown, new, change.
  - Shared relay helpers: `request_relay`,
    `request_relay_with_namespace`, `map_relay_stream_failure`,
    `associated_namespace`.
  - `#[tool_handler(router = self.tool_router)] impl
    rmcp::ServerHandler for McpServer` — provides the MCP `get_info`
    capability advertisement and tool dispatch.
  - `pub async fn run(configuration: McpConfiguration) -> Result<()>`
    — stdio bootstrap; blocks until shutdown.
- `handlers/`
  - Per-tool `#[tool_router]` impl blocks. See
    `handlers/README.md` for the per-file pattern, the list of
    files, and the tool-to-relay-request mapping.

## State flow

`McpConfiguration` is provided once at startup. `McpServer::new()`
materializes the `RelayStreamSession` when both `sender_session` and
`associated_bundle_paths` are configured; otherwise the
`relay_stream` slot stays `None`. That single precondition —
`relay_stream.is_some()`, equivalently association of both fields — is
what every relay-backed tool needs, so an unassociated server rejects
each of them with one actionable, validation-shaped
`validation_unassociated_server` error (built by
`errors::unassociated_server_error`, reached via the early
`require_associated_sender_session` / `require_association` prechecks
and the `map_relay_stream_failure` chokepoint). Genuine IO failures on
an associated server still classify as
`relay_timeout` / `relay_unavailable` / `internal_unexpected_failure`
via `validation::is_relay_unavailable_error`.

The `Arc<McpState>` is shared across all per-tool handler
invocations on the same `McpServer` instance, so a single server
handles all incoming requests with a single cached relay stream.
That single-stream invariant is what allows `list.principals` with
`namespace="*"` to fan out across bundle relays: each fan-out call
reuses the same `RelayStreamSession` for the corresponding
`@<bundle>` wire-envelope namespace, avoiding a fresh Hello per
target (which would conflict with the cached registration under
`runtime_identity_claim_conflict`).

## Capability advertisement

`#[tool_handler(router = self.tool_router)]` reads the composed
`ToolRouter<Self>` field rather than recomputing the per-call
dispatch table. The `ServerInfo` advertises tool support plus an
association-specific instruction string: the base `agentmux MCP server
for tmux-backed multi-agent coordination.` line, extended with the
associated namespace/session when associated, or the
`validation_unassociated_server` remedy when not, so a client sees the
server's association at `initialize` time.
