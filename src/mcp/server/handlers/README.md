# MCP Handlers

One file per MCP tool, each holding an `impl McpServer` block with
a named `#[tool_router]` attribute. The router-builder functions
(`tool_router_X`) are composed in `McpServer::new()` (in
`server/service.rs`).

## Pattern

Every file follows the same shape:

```rust
#[tool_router(router = tool_router_X, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "<tool>",
        description = "<one-line description surfaced in MCP tool list>"
    )]
    async fn tool_X(
        &self,
        Parameters(params): Parameters<XParams>,
    ) -> Result<CallToolResult, McpError> {
        // 1. validate_X_request(&params)?
        // 2. emit_inscription("mcp.tool.X.request", ...)?
        // 3. resolve requester_session from self.state.configuration.sender_session
        //    (or surface validation_unknown_sender if the MCP server is unassociated)
        // 4. build a RelayRequest
        // 5. self.request_relay(&request) -> match on RelayResponse:
        //    - success variant -> map to JSON, emit "mcp.tool.X.success"
        //    - RelayResponse::Error -> map_relay_error, emit "mcp.tool.X.relay_error"
        //    - other variant -> internal_tool_error("internal_unexpected_failure")
        //    - io error -> self.map_relay_stream_failure("mcp.tool.X.io_error", source)
    }
}
```

The `pub(super) state: Arc<McpState>` field on `McpServer` (defined
in `server/service.rs`) is visible from these sibling impl blocks.
The `McpServer` import path is `crate::mcp::server::McpServer`
(`pub(crate)` from `server/mod.rs`).

## Tool inventory

| File | Tool | Meta-commands | Maps to |
| --- | --- | --- | --- |
| `list.rs` | `list` | `principals`, `decisions` | `RelayRequest::List`, `RelayRequest::ChoicesList` |
| `help.rs` | `help` | — | `crate::mcp::help::help_tool` |
| `send.rs` | `send` | — | `RelayRequest::Send` |
| `look.rs` | `look` | — | `RelayRequest::Look` |
| `raww.rs` | `raww` | — | `RelayRequest::Raww` |
| `choose.rs` | `choose` | — | `RelayRequest::ChoicesPick` |
| `updown.rs` | `updown` | `up`, `down` | `RelayRequest::Up`, `RelayRequest::Down` |
| `new.rs` | `new` | `peer` | `RelayRequest::NewPeer` |
| `change.rs` | `change` | `psk` | `RelayRequest::ChangePsk` |

The full parameter shapes for each tool are documented in
`src/mcp/params.rs` and surfaced via the `help` tool at runtime
(`help query="<tool>"` for the JSON schema, or `help
query="<tool>.<command>"` for the meta-tool arg schema).

## Per-handler responsibilities

In addition to the shared pattern above, each handler owns the
tool-specific quirks:

- `list.rs` — the largest handler; owns `list_principals` and
  `list_decisions` dispatch, the `list_sessions_single_bundle` /
  `list_sessions_all_bundles` / `list_global_sessions` helpers
  (fan-out order, GLOBAL envelope pinning, synthetic down-bundle
  fallback for an unreachable home bundle, synthetic empty
  `GLOBAL` bundle on relay-unavailable), and the
  `synthesize_down_bundle` / `synthesize_empty_global_bundle`
  constructors. Always appends a `GLOBAL` view to the response.
- `send.rs` — calls `qualify_send_targets` after sender resolution
  (so an unidentified sender fails as `validation_unknown_sender`
  regardless of target shape). The qualified target list is what
  rides the relay.
- `look.rs` — does not qualify `target_session`; the caller must
  supply the `@<namespace>` suffix. Maps `LookSnapshotPayload` to
  the JSON shape described in `src/mcp/README.md` (Responsibilities
  section).
- `updown.rs`, `new.rs`, `change.rs` — own the meta-tool
  `command` dispatch (`up` / `down`, `peer`, `psk` respectively)
  and the typed request builders.
- `choose.rs` — the `decisions` companion to `list.decisions`. The
  decision actor is association-derived by the relay, so
  caller-supplied sender-like identity fields are rejected as
  unknown parameters by the struct-level `extra_fields` capture.
- `raww.rs` — minimal; raw text passthrough with `no_enter` and
  `request_id` plumbing.
- `help.rs` — thin wrapper around `crate::mcp::help::help_tool`.
  Does not reach the relay.
