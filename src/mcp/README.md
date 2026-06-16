# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list` (requires `command="principals"` or `command="decisions"`; for
    `principals`, `args.namespace` selects the scope: omitted/home bundle, a
    bundle name, `GLOBAL`, or `*` for fan-out; `decisions` lists the bundle's
    pending ACP choice queue)
  - `help`
  - `look`
  - `choose` (submit an ACP-native choice decision)
  - `updown` (requires `command="up"` or `command="down"`)
  - `new` (requires `command="peer"`)
  - `change` (requires `command="psk"`)
  - `raww`
  - `send`
- Preserve canonical relay `list` and `look` success payloads without adapter
  reshaping:
  - `list`: relay `ListedBundle` (including the `hosted` flag and per-session
    `ready` flag) passes through unchanged; synthesized down-bundle payloads for
    the unreachable home bundle report `hosted: false` and `ready: false` for
    every configured session.
  - `look` tmux: `snapshot_format="lines"` + `snapshot_lines`
  - `look` ACP: `snapshot_format="acp_entries_v1"` + `snapshot_entries`
    (+ freshness fields), plus `entries_total` (full replay-buffer size) and
    `returned_entries_count` (entries in this window) so callers can detect
    truncation and bound backward walks.
  - `look` windowing parameters (both forwarded to the relay):
    - `lines` (optional): window size. tmux pane lines (default 120) or ACP
      replay entries (default 50 — ACP entries are far larger than tmux lines,
      so the smaller default keeps responses under the MCP payload limit).
    - `offset` (optional): entries to skip from the newest end before the
      tail-N window, for walking backward through older ACP context. ACP
      targets only; a non-zero `offset` against a tmux target is rejected with
      `validation_offset_unsupported`.
  - `send` and `look` both surface sender-attribution fields when the relay
    populates them:
    - `authenticated_identity` (optional): the relay-verified `principal_id`
      of the caller, present only for store-backed credential connections and
      omitted for socket-trust sessions.
    - `on_behalf_of` (optional): the delegated principal the caller is acting
      for. Reserved for a future delegated-identity delta; surfaced in the
      schema so callers can consume it without a breaking change.
    Both follow the relay's `skip_serializing_if` semantics: omitted entirely
    (not `null`) when absent.
- Validate MCP request payloads.
- Forward valid requests to relay over the bundle Unix socket.
- Preserve relay error taxonomy/details when relay returns structured errors.
- Emit MCP inscriptions for request, success, and error events.

## Data Flow

1. MCP client calls `list`, `look`, `choose`, `updown`, `raww`, or `send`.
2. MCP client can call `help` to discover tool/command schemas.
3. `src/mcp/mod.rs` validates parameter shape and transport-compatible options.
4. Request is forwarded as relay contract:
   - `list` (`command="principals"`) -> `RelayStreamSession` (`RelayRequest::List`)
   - `look` -> `RelayStreamSession` (`RelayRequest::Look`)
   - `list` (`command="decisions"`) -> `RelayStreamSession`
     (`RelayRequest::ChoicesList`)
   - `choose` -> `RelayStreamSession` (`RelayRequest::ChoicesPick`)
   - `updown` (`command="up"`) -> `RelayStreamSession` (`RelayRequest::Up`)
   - `updown` (`command="down"`) -> `RelayStreamSession` (`RelayRequest::Down`)
   - `new` (`command="peer"`) -> `RelayStreamSession` (`RelayRequest::NewPeer`)
   - `change` (`command="psk"`) -> `RelayStreamSession`
     (`RelayRequest::ChangePsk`)
   - `raww` -> `RelayStreamSession` (`RelayRequest::Raww`)
   - `send` -> `RelayStreamSession` (`RelayRequest::Send`)
5. For `namespace="*"`, MCP performs adapter fanout across bundle relays in
   deterministic lexicographic order.
6. Relay response is mapped back to MCP JSON payload.

## Bundle Updown

- The `updown` tool administers bundle runtime state for the associated
  bundle:
  - `command="up"` requests `RelayRequest::Up` (host the bundle).
  - `command="down"` requests `RelayRequest::Down` (unhost the bundle).
- Requests ride the MCP server's long-lived `RelayStreamSession`; the relay
  authorizes the caller-session principal carried by the Hello-established
  connection against the `updown` policy control (deny by default).
- The MCP tool only addresses the MCP server's associated bundle; cross-bundle
  administration requires a separate relay connection and is out of scope.
- Relay returns `RelayResponse::BundleTransition` on success; the MCP response
  forwards `schema_version`, `action`, `bundles`, `changed_bundle_count`,
  `skipped_bundle_count`, `failed_bundle_count`, and `changed_any` unchanged.
- Relay `authorization_forbidden` (capability `updown`) surfaces as a typed
  MCP tool error with the relay error code and details preserved.

## Choice Decisions

- The relay ACP choice queue is split across two tools:
  - `list` with `command="decisions"` polls the bundle-scoped pending-request
    set; the response payload mirrors the `choices.requested` event fields
    exactly.
  - `choose` submits a decision (`outcome="selected"` requires `option_id`;
    `outcome="cancelled"` rejects `option_id`).
- `choose` rejects caller-supplied sender-identity fields (`decided_by`,
  `ui_session_id`, `operator_session_id`) before relay submission; the deciding
  identity is association-derived and relay-stamped.
- Both surfaces are gated solely on the sender session's `choose` policy
  capability (the transport's `gives_choices` flag is not required). The MCP
  server forwards the request over its relay stream, and the relay authorizes
  it against the bundle policy preset. Sessions whose policy does not enable
  `choose` receive the relay submitter-gate rejection.

## Credential Administration

- The `new` tool (`command="peer"`) registers a principal and mints its PSK:
  the relay generates the PSK, stores only its SHA-256 hash, and returns the raw
  value once (or, with `args.output_path`, writes it to that absolute path and
  omits it from the response). `args.scope` is recorded on the principal
  (set for `@RELAY`/`@EXTERNAL`).
- The `change` tool (`command="psk"`) rotates an existing principal's PSK and
  returns the new value; Slice 1 is a store update only (no revocation dispatch).
- Both are relay-wide operations: they ride the MCP server's relay stream, and
  the relay authorizes the connection's principal against its policy preset
  relay-wide, requiring an `all` `new.peer` / `change.psk` grant. A
  bundle-relative `home` grant is insufficient. The MCP server's own
  identity must therefore carry an operator policy for these tools to succeed.

## Key Types

- `McpConfiguration`
  - startup configuration for runtime roots, bundle paths, and sender session
    identity.
- `McpServer`
  - tool router + handlers.
- `SendParams`
  - MCP `send` request schema.

## Module Layout

- `mod.rs`
  - Module declarations and public re-exports.
- `server/`
  - Pure directory module holding the MCP server surface.
- `server/mod.rs`
  - Operator-hub: only `mod` declarations and re-exports. Re-exports
    `McpConfiguration` and `run` from `core`.
- `server/core.rs`
  - `McpConfiguration`, `McpState`, `McpServer`, the shared relay helpers
    (`request_relay`, `request_relay_with_namespace`, `map_relay_stream_failure`),
    the `#[tool_handler] impl rmcp::ServerHandler for McpServer`, and
    `pub async fn run`. `McpServer::new()` composes the per-tool routers
    via the `Add` impl on `ToolRouter`.
- `server/handlers/`
  - One file per MCP tool: `list.rs`, `help.rs`, `send.rs`, `look.rs`,
    `raww.rs`, `choose.rs`, `updown.rs`, `new.rs`, `change.rs`. Each file
    holds an `impl McpServer` block with a named `#[tool_router(router = tool_router_X, vis = "pub(crate)")]`
    attribute, the `#[tool]` method, and the tool's helper methods. The
    `pub(super)` `state` field on `McpServer` (defined in `core.rs`) is
    visible from these sibling impl blocks.
- `params.rs`
  - MCP tool parameter and meta-tool argument schemas plus shared command
    constants.
- `validation.rs`
  - request-shape validation, meta-tool argument parsing, and relay I/O error
    classification helpers.
- `help.rs`
  - help catalog responses and generated JSON schemas.
- `errors.rs`
  - relay/configuration/runtime error mapping and MCP error payload helpers.

## Validation and Error Policy

- MCP rejects invalid request shapes before relay submission (for example empty
  targets or conflicting timeout fields).
- MCP rejects unknown top-level tool fields and unknown meta-tool `args` fields
  with `validation_invalid_params`; error details include the rejected field
  paths (for example `args.bundle_name_typo`).
- Help schemas set `additionalProperties=false` for documented request shapes.
- MCP does not perform shadow authorization checks.
- Relay `authorization_forbidden` and other relay codes are passed through as
  MCP errors with relay details.

## Event Handling Note

- Relay may return stream events alongside direct responses.
- Current MCP behavior logs these events via inscriptions and ignores them at
  tool-response level.
