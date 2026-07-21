# Change: Add cross-relay list discovery

## Why

MCP meta-tool command selectors are plain strings, so weaker tool-call
constructors get no JSON Schema enum signal for valid subcommands and sometimes
emit invalid dispatch values. Cross-relay `Send`/`Raww` has also shipped without
an operator discovery path for the configured relay aliases, namespaces, and
principals needed to construct foreign targets.

## What Changes

- Constrain MCP meta-tool `command` fields to generated JSON Schema enums for
  `list`, `updown`, `new`, and `change`.
- Add `list` with `command="relays"` to enumerate locally configured outbound
  relay aliases without dialing them.
- Add `list` with `command="namespaces"` and an optional `relay` argument to
  discover namespaces on the local relay or one configured foreign relay.
- Extend `list` with `command="principals"` so optional `args.relay` selects a
  configured foreign relay. Foreign principal listing requires a concrete
  `args.namespace`; local listing keeps its existing namespace semantics.
- Apply two authorization boundaries to foreign discovery: the origin
  requester's `list` control must permit cross-relay reach, and the receiving
  relay filters or denies the request using the authenticated peer relay
  principal's registered ingress `scope`.
- Mark scope-filtered bundle results with `principals_partial=true` so callers
  can distinguish complete listings from principal-scoped subsets.
- Keep cross-relay `Look`, multi-hop relay discovery, and capability-specific
  peer ingress controls out of scope.

## Impact

- Affected specs:
  - `mcp-tool-surface` — enum-constrained command schemas, `list.relays`,
    `list.namespaces`, and the `list.principals` relay selector.
  - `cross-relay-routing` — namespace/principal discovery forwarding and typed
    response/error propagation over configured peer connections.
  - `relay-routing-layer` — origin `list` authorization and receiving-relay
    ingress filtering for cross-relay discovery.
- Affected code: `src/mcp/params.rs`, `src/mcp/help.rs`,
  `src/mcp/server/handlers/list.rs`, `src/mcp/validation.rs`,
  `src/relay/contract.rs`, `src/relay/peer_connection.rs`, `src/relay/mod.rs`,
  `src/relay/connection/mod.rs`, `src/relay/handlers/listing.rs` (reuse the
  canonical listed-bundle builder), a new
  `src/relay/handlers/discovery.rs`, `src/relay/authorization/checks.rs`,
  `src/runtime/inscriptions.rs`, and MCP/relay integration tests.
- No `relay.toml` shape change: existing outbound `[[peers]]` entries and peer
  credential files remain the origin-side peer configuration.
