# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list` (MVP requires `command="sessions"`)
  - `help`
  - `look`
  - `grant` (requires `command="list"` or `command="resolve"`)
  - `lifecycle` (requires `command="up"` or `command="down"`)
  - `raww`
  - `send`
- Preserve canonical relay `look` success payloads without adapter reshaping:
  - tmux: `snapshot_format="lines"` + `snapshot_lines`
  - ACP: `snapshot_format="acp_entries_v1"` + `snapshot_entries` (+ freshness fields)
- Validate MCP request payloads.
- Forward valid requests to relay over the bundle Unix socket.
- Preserve relay error taxonomy/details when relay returns structured errors.
- Emit MCP inscriptions for request, success, and error events.

## Data Flow

1. MCP client calls `list`, `look`, `grant`, `lifecycle`, `raww`, or `send`.
2. MCP client can call `help` to discover tool/command schemas.
3. `src/mcp/mod.rs` validates parameter shape and transport-compatible options.
4. Request is forwarded as relay contract:
   - `list` (`command="sessions"`) -> one-shot `request_relay` probes (`RelayRequest::List`)
   - `look` -> `RelayStreamSession` (`RelayRequest::Look`)
   - `grant` (`command="list"`) -> `RelayStreamSession`
     (`RelayRequest::PermissionList`)
   - `grant` (`command="resolve"`) -> `RelayStreamSession`
     (`RelayRequest::PermissionResolve`)
   - `lifecycle` (`command="up"`) -> `RelayStreamSession` (`RelayRequest::Up`)
   - `lifecycle` (`command="down"`) -> `RelayStreamSession` (`RelayRequest::Down`)
   - `raww` -> `RelayStreamSession` (`RelayRequest::Raww`)
   - `send` -> `RelayStreamSession` (`RelayRequest::Send`)
5. For `all=true`, MCP performs adapter fanout across bundle relays in
   deterministic lexicographic order.
6. Relay response is mapped back to MCP JSON payload.

## Bundle Lifecycle

- The `lifecycle` tool administers bundle runtime state for the associated
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

## Permission Granting

- The `grant` tool exposes the relay ACP permission queue:
  - `command="list"` polls the bundle-scoped pending-request set; the response
    payload mirrors the `permission.requested` event fields exactly.
  - `command="resolve"` submits a decision (`outcome="selected"` requires
    `option_id`; `outcome="cancelled"` rejects `option_id`).
- MCP rejects decider-identity fields (`decided_by`, `ui_session_id`,
  `operator_session_id`) before relay submission; the deciding identity is
  association-derived and relay-stamped.
- Both subcommands are gated solely on the sender session's `grant` policy
  capability. The MCP server forwards the decision over its relay stream, and
  the relay authorizes it against the bundle policy preset. Sessions whose
  policy does not enable `grant` receive the relay submitter-gate rejection.

## Key Types

- `McpConfiguration`
  - startup configuration for runtime roots, bundle paths, and sender session
    identity.
- `McpServer`
  - tool router + handlers.
- `SendParams`
  - MCP `send` request schema, including optional transport-scoped timeout
    overrides (`quiescence_timeout_ms`, `acp_turn_timeout_ms`).

## Module Layout

- `mod.rs`
  - Module declarations and public re-exports.
- `server.rs`
  - MCP server state, tool handlers, and relay/list plumbing.
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
