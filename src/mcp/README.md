# MCP Surface

This module implements the MCP stdio server for `agentmux`.

## Responsibilities

- Advertise and handle MCP tools:
  - `list` (requires `command="principals"`, `command="namespaces"`,
    `command="relays"`, or `command="decisions"`; for `principals`,
    `args.namespace` selects the scope: omitted/home bundle, a bundle name,
    `GLOBAL`, or `*` for fan-out, and an optional `args.relay` forwards the
    lookup to one configured foreign relay — foreign listing requires a
    concrete `args.namespace`; `namespaces` enumerates local namespaces or,
    with an optional `args.relay`, one configured foreign relay's; `relays`
    enumerates locally configured outbound relay aliases without dialing them;
    `decisions` lists the bundle's pending ACP choice queue)
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
  - `list`: relay `ListedBundle` (including the `hosted` flag, per-session
    `ready` flag, `state_reason_code` / `state_reason`, and `startup_failure_*`
    fields) passes through unchanged. For the unreachable home bundle the
    server synthesizes a `Down` bundle with `hosted: false` and `ready: false`
    on every configured session, and a relay-unavailable `GLOBAL` view
    synthesizes an empty `Down` bundle. Both synthetic shapes carry
    `state_reason_code` / `state_reason` so callers can distinguish
    `not_started` (no socket) from `relay_unavailable` (socket present but
    relay not responding).
  - `look` tmux: `snapshot_format="lines"` + `snapshot_lines`.
  - `look` ACP: `snapshot_format="structured_entries_v1"` + `snapshot_entries`
    plus `entries_total` (full replay-buffer size), `returned_entries_count`
    (entries in this window — callers can detect truncation and bound
    backward walks), `freshness`, `snapshot_source`, `stale_reason_code`, and
    `snapshot_age_ms` (the last two are only populated when the snapshot is
    degraded).
  - `look` windowing parameters (both forwarded to the relay):
    - `lines` (optional): window size. tmux pane lines (default 120) or ACP
      replay entries (default 50 — ACP entries are far larger than tmux lines,
      so the smaller default keeps responses under the MCP payload limit).
      Validated locally to be in `[1, 1000]`; out-of-range values surface as
      `validation_invalid_lines` with the rejected `lines` and the configured
      `min` / `max` bounds in the details.
    - `offset` (optional): entries to skip from the newest end before the
      tail-N window, for walking backward through older ACP context. ACP
      targets only; a non-zero `offset` against a tmux target is rejected
      with `validation_offset_unsupported`.
  - `send` and `look` both surface sender-attribution fields when the relay
    populates them:
    - `authenticated_identity` (optional): the relay-verified `principal_id`
      of the caller, present only for store-backed credential connections
      and omitted for socket-trust sessions.
    - `on_behalf_of` (optional): the delegated principal the caller is
      acting for. Reserved for a future delegated-identity delta; surfaced
      in the schema so callers can consume it without a breaking change.
    Both follow the relay's `skip_serializing_if` semantics: omitted
    entirely (not `null`) when absent.
- Validate MCP request payloads before relay submission.
- Forward valid requests to the relay over the bundle Unix socket, using a
  single long-lived `RelayStreamSession` (Hello-registered at server
  construction) so per-tool calls share the connection. Fan-out for
  `list.principals` with `namespace="*"` reuses that same connection per
  bundle relay.
- Preserve relay error taxonomy / details when relay returns structured
  errors.
- Emit MCP inscriptions for request, success, and error events. Every tool
  emits `mcp.tool.<name>.<phase>` (request / success / relay_error /
  unexpected_response / io_error) plus `mcp.tool.stream.events_ignored`
  when the relay returns stream events that the MCP layer does not
  surface.

## Data Flow

1. MCP client calls `list`, `look`, `choose`, `updown`, `raww`, or `send`.
2. MCP client can call `help` to discover tool and command schemas. The
   `help` tool does not reach the relay — it composes a JSON response
   from the in-memory tool catalog and the parameter schemas in
   `params.rs`.
3. `src/mcp/validation.rs` rejects invalid request shapes (empty
   `target_session`, empty `message` on `send`, conflicting
   `broadcast` / `targets` on `send`, lines outside `[1, 1000]` on
   `look`, etc.) before any relay work begins.
4. The per-tool handler in `src/mcp/server/handlers/<tool>.rs` emits a
   `mcp.tool.<tool>.request` inscription, resolves the requester session
   via `require_associated_sender_session` (or surfaces
   `validation_unassociated_server` if the MCP server is unassociated),
   and builds a `RelayRequest`.
5. `send`, `look`, and `raww` qualify their targets through the shared
   `qualify_target` helper, filling in the `@<namespace>` suffix the relay
   requires: a target that already carries `@<namespace>` passes through
   verbatim (so a `<session>@<peer-bundle>` target still reaches a peer
   bundle); a bare target is qualified with the MCP server's bound bundle.
   Qualification runs only after the step-4 association precheck, so on an
   unassociated server the call is already rejected with
   `validation_unassociated_server` and never reaches qualification.
   (`send` qualifies its whole target list via `qualify_send_targets`,
   which maps `qualify_target` over each entry.)
6. Request is forwarded as a relay contract:
   - `list` (`command="principals"`) -> `RelayStreamSession`
     (`RelayRequest::List`). For `args.namespace="*"` the MCP server
     fans out across configured bundle relays in deterministic
     lexicographic order via `list_sessions_all_bundles`; a `GLOBAL`
     namespace routes through `list_global_sessions`, which pins the
     wire-envelope namespace and surfaces a synthetic empty `Down`
     bundle when the relay is unavailable. A `GLOBAL` view is appended
     to the response regardless of the requested namespace.
   - `list` (`command="namespaces"`) -> `RelayStreamSession`
     (`RelayRequest::DiscoverNamespaces`); with `args.relay` set the
     origin-local alias rides the request so the relay forwards to that peer.
   - `list` (`command="relays"`) -> `RelayStreamSession`
     (`RelayRequest::ListRelays`)
   - `list` (`command="principals"`, `args.relay` set) -> `RelayStreamSession`
     (`RelayRequest::DiscoverPrincipals`) for foreign discovery, in place of
     the local `RelayRequest::List` path.
   - `list` (`command="decisions"`) -> `RelayStreamSession`
     (`RelayRequest::ChoicesList`)
   - `look` -> `RelayStreamSession` (`RelayRequest::Look`)
   - `choose` -> `RelayStreamSession` (`RelayRequest::ChoicesPick`)
   - `updown` (`command="up"`) -> `RelayStreamSession`
     (`RelayRequest::Up`)
   - `updown` (`command="down"`) -> `RelayStreamSession`
     (`RelayRequest::Down`)
   - `new` (`command="peer"`) -> `RelayStreamSession`
     (`RelayRequest::NewPeer`)
   - `change` (`command="psk"`) -> `RelayStreamSession`
     (`RelayRequest::ChangePsk`)
   - `raww` -> `RelayStreamSession` (`RelayRequest::Raww`)
   - `send` -> `RelayStreamSession` (`RelayRequest::Send`)
7. Relay response is mapped back to MCP JSON payload. The handler
   matches on the `RelayResponse` variant: the success variant is
   mapped to a JSON object (the relay's payload preserved per the
   Responsibilities section); `RelayResponse::Error` flows through
   `map_relay_error`; any other variant triggers
   `internal_unexpected_failure` with the `response` shape preserved
   in the details. IO errors are routed through
   `map_relay_stream_failure`, which branches on association: an
   associated server maps a genuine relay IO failure to the typed
   `relay_timeout` / `relay_unavailable` / `internal_unexpected_failure`
   codes (with the relay socket path and `io_error_kind` in the
   details), while an unassociated server — whose absent stream is the
   cause — returns the validation-shaped `validation_unassociated_server`
   instead.

## Bundle Updown

- The `updown` tool administers bundle runtime state for the associated
  bundle:
  - `command="up"` requests `RelayRequest::Up` (host the bundle).
  - `command="down"` requests `RelayRequest::Down` (unhost the bundle).
- Requests ride the MCP server's long-lived `RelayStreamSession`; the
  relay authorizes the caller-session principal carried by the
  Hello-established connection against the `updown` policy control
  (deny by default).
- The MCP tool only addresses the MCP server's associated bundle;
  cross-bundle administration requires a separate relay connection
  and is out of scope.
- Relay returns `RelayResponse::BundleTransition` on success; the MCP
  response forwards `schema_version`, `action`, `bundles`,
  `changed_bundle_count`, `skipped_bundle_count`,
  `failed_bundle_count`, and `changed_any` unchanged.
- Relay `authorization_forbidden` (capability `updown`) surfaces as a
  typed MCP tool error with the relay error code and details preserved.

## Choice Decisions

- The relay ACP choice queue is split across two tools:
  - `list` with `command="decisions"` polls the bundle-scoped
    pending-request set; the response payload mirrors the
    `choices.requested` event fields exactly.
  - `choose` submits a decision (`outcome="selected"` requires
    `option_id`; `outcome="cancelled"` rejects `option_id`).
- `choose` rejects caller-supplied sender-identity fields
  (`decided_by`, `ui_session_id`, `operator_session_id`) before relay
  submission; the deciding identity is association-derived and
  relay-stamped.
- Both surfaces are gated solely on the sender session's `choose`
  policy capability (the transport's `can_give_choices` flag is not
  required). The MCP server forwards the request over its relay
  stream, and the relay authorizes it against the bundle policy
  preset. Sessions whose policy does not enable `choose` receive the
  relay submitter-gate rejection.

## Credential Administration

- The `new` tool (`command="peer"`) registers a principal and mints
  its PSK: the relay generates the PSK, stores only its SHA-256 hash,
  and returns the raw value once (or, with `args.output_path`, writes
  it to that absolute path and omits it from the response). `args.scope`
  is recorded on the principal (set for `@RELAY` / `@EXTERNAL`).
- The `change` tool (`command="psk"`) rotates an existing principal's
  PSK and returns the new value; Slice 1 is a store update only
  (no revocation dispatch).
- Both are relay-wide operations: they ride the MCP server's relay
  stream, and the relay authorizes the connection's principal
  against its policy preset relay-wide, requiring an `all`
  `new.peer` / `change.psk` grant. A bundle-relative `home` grant is
  insufficient. The MCP server's own identity must therefore carry
  an operator policy for these tools to succeed.

## Cross-Relay Discovery

- Three `list` commands expose the discovery surface operators need to
  construct foreign `<session>@<bundle>!<alias>` targets for cross-relay
  `send` / `raww`:
  - `list.relays` enumerates the locally configured outbound relay aliases
    (from `relay.toml` `[[peers]]`) in sorted order. It never dials a peer and
    never exposes address, `connect-as`, or credential detail — only the alias
    the local relay uses. Returns an empty array when no peers are configured.
  - `list.namespaces` enumerates namespaces on the local relay, or — with
    `args.relay` set to a configured alias — forwards a single hop to that peer
    and returns the namespaces its ingress scope permits.
  - `list.principals` with `args.relay` set forwards principal discovery to
    that peer for a required concrete `args.namespace` (no `*`, reserved, or
    empty namespace). Without `args.relay` it keeps its existing local
    semantics.
- These commands ride the MCP server's relay stream like every other tool, but
  the relay dispatches them relay-wide (like identity administration) rather
  than through a bundle. The MCP layer performs no authorization: both the
  origin `list`-scope check and the receiving relay's ingress filtering happen
  on the relay (see `src/relay/README.md`, Cross-Relay Discovery).
- Foreign `list.principals` responses may carry `principals_partial=true` on a
  bundle whose principal set was narrowed by the peer's ingress scope, so a
  caller can tell a scope-filtered subset from a complete listing. The MCP
  server passes the flag through unchanged (omitted, not `null`, when absent).
- Each command emits the standard five lifecycle inscriptions
  (`mcp.tool.list.relays.*`, `mcp.tool.list.namespaces.*`,
  `mcp.tool.list.principals.*`: request / success / relay_error /
  unexpected_response / io_error).

## Key Types

- `McpConfiguration`
  - startup configuration for runtime roots, bundle paths, and sender
    session identity. Constructed once at `McpServer::new()` and held
    in `McpState.configuration`.
- `McpState`
  - runtime state shared across all per-tool handlers on a single
    `McpServer` instance: the original `McpConfiguration` plus a
    `Mutex<Option<RelayStreamSession>>`. The `relay_stream` slot is
    `Some` when both `sender_session` and `associated_bundle_paths`
    were configured at server construction (the normal
    bundle-associated case) and `None` for relay-wide (unassociated)
    MCP servers.
- `McpServer`
  - `pub(crate) state: Arc<McpState>` plus a composed
    `ToolRouter<Self>`. The router is built once in
    `McpServer::new()` by adding the nine per-tool routers in
    registration order — `tool_router_list + tool_router_help +
    tool_router_send + tool_router_look + tool_router_raww +
    tool_router_choose + tool_router_updown + tool_router_new +
    tool_router_change` — via the `Add` impl on `ToolRouter`.
- `SendParams`, `ListParams`, `LookParams`, `ChooseParams`,
  `UpdownParams`, `NewParams`, `ChangeParams`, `RawwParams`,
  `HelpParams`
  - MCP tool parameter and meta-tool argument schemas. See
    `params.rs` for the full shape, and `help query="<tool>"` for
    the runtime-rendered JSON schema. Each struct uses
    `#[schemars(deny_unknown_fields)]` and a flatten-captured
    `extra_fields: BTreeMap<String, Value>` so unknown fields are
    surfaced as `validation_invalid_params` rather than silently
    dropped.

## Module Layout

- `mod.rs`
  - Module declarations and public re-exports.
- `server/`
  - Pure directory module holding the MCP server surface. See
    `server/README.md` for the directory's contract.
- `server/mod.rs`
  - Operator-hub: only `mod` declarations and re-exports. Re-exports
    `McpConfiguration` and `run` from `service`; `McpServer` is
    `pub(crate)`-visible to the per-tool handlers in `handlers/`.
- `server/service.rs`
  - `McpConfiguration`, `McpState`, `McpServer`, the shared relay
    helpers (`request_relay`, `request_relay_with_namespace`,
    `map_relay_stream_failure`, `associated_namespace`), the
    `#[tool_handler(router = self.tool_router)] impl
    rmcp::ServerHandler for McpServer` block (provides the MCP
    `get_info` capability advertisement and tool dispatch), and
    `pub async fn run`. `McpServer::new()` composes the per-tool
    routers via the `Add` impl on `ToolRouter`.
- `server/handlers/`
  - One file per MCP tool: `list.rs`, `help.rs`, `send.rs`,
    `look.rs`, `raww.rs`, `choose.rs`, `updown.rs`, `new.rs`,
    `change.rs`. See `handlers/README.md` for the per-tool pattern
    and tool inventory. The `pub(super) state` field on
    `McpServer` (defined in `service.rs`) is visible from these
    sibling impl blocks.
- `params.rs`
  - MCP tool parameter and meta-tool argument schemas plus shared
    command constants.
- `validation.rs`
  - request-shape validation, meta-tool argument parsing,
    `qualify_send_targets` for `send`, and relay I/O error
    classification helpers (`is_relay_unavailable_error`,
    `is_relay_timeout_error`).
- `help.rs`
  - help catalog responses and generated JSON schemas. Composed at
    runtime by `help_tool(params, association)` and surfaced through
    the `help` MCP tool; the top-level catalog carries an
    `association` block (associated namespace/session, or the
    unassociated remedy) so a caller can discover an unassociated
    server before invoking a relay-backed tool.
- `errors.rs`
  - relay / configuration / runtime error mapping and MCP error
    payload helpers. The MCP error payload shape is
    `{code, message, details}` for both `validation_tool_error` and
    `internal_tool_error`.

## Validation and Error Policy

- MCP rejects invalid request shapes before relay submission (for
  example empty `target_session`, empty `message` on `send`,
  conflicting `broadcast` / `targets` on `send`, `lines` outside
  `[1, 1000]` on `look`).
- MCP rejects unknown top-level tool fields and unknown meta-tool
  `args` fields with `validation_invalid_params`; error details
  include the rejected field paths (for example
  `args.bundle_name_typo`).
- `choose` enforces the `selected` / `cancelled` outcome contract: a
  `selected` outcome without an `option_id` is rejected with
  `validation_invalid_params`; a `cancelled` outcome with an
  `option_id` is rejected with `validation_invalid_params`; caller
  -supplied sender-identity fields (`decided_by`, `ui_session_id`,
  `operator_session_id`) are rejected as unknown parameters.
- `send`, `look`, and `raww` qualify a bare target with the MCP server's
  bound bundle via the shared `qualify_target` helper (`send` maps it over
  its target list as `qualify_send_targets`). Qualification runs after the
  association precheck, so an unassociated server is already rejected with
  `validation_unassociated_server` (below) before a bare target could reach
  qualification.
- A relay-backed tool called on an unassociated MCP server (one
  started without a resolvable bundle+session, so `relay_stream` is
  `None`) is rejected with one validation-shaped
  `validation_unassociated_server` error carrying
  `{reason: "unassociated_server", remedy}`. Both the early
  `require_associated_sender_session` / `require_association` prechecks
  and the `map_relay_stream_failure` chokepoint route through
  `errors::unassociated_server_error`, so every relay-backed tool
  speaks one language instead of the former
  `validation_unknown_sender` / `internal_unexpected_failure` split.
- Help schemas set `additionalProperties=false` for documented
  request shapes.
- MCP does not perform shadow authorization checks.
- Relay `authorization_forbidden` is surfaced as a validation-shaped
  MCP tool error (preserving the relay code and details) via
  `map_relay_error`. Other `validation_*` relay codes pass through
  the same path. Non-validation relay codes surface as
  `internal_unexpected_failure` with the relay code in the
  details.
- Relay stream I/O errors are classified by
  `map_relay_request_failure`:
  - `std::io::ErrorKind::TimedOut` -> `relay_timeout` (with
    `relay_socket`, `io_error_kind`, and `cause` in details).
  - `std::io::ErrorKind::{NotFound, ConnectionRefused,
    ConnectionAborted, ConnectionReset, BrokenPipe,
    UnexpectedEof}` -> `relay_unavailable` (with the same detail
    fields).
  - everything else -> `internal_unexpected_failure` with
    `relay_socket`, `io_error_kind`, and `cause` in details.

## Event Handling Note

- Relay may return stream events alongside direct responses.
- Current MCP behavior logs these events via the
  `mcp.tool.stream.events_ignored` inscription (with `namespace`
  and `count`) and ignores them at tool-response level.
