# Change: Add suffix-based target routing and GLOBAL namespace delivery

## Why

Two problems need to be fixed together:

1. **Design correction**: MCP tasks 2.1–2.2 in `rename-request-routing-field`
   added a `namespace` parameter to `look`, `raww`, and `send` tool schemas.
   This creates two competing routing mechanisms. The routing context for
   per-target operations is already fully encoded in each target's
   `@<namespace>` suffix; an explicit `namespace` parameter is redundant and
   misleading. It should not exist on these tools.

2. **Functional gap**: `namespace = "GLOBAL"` on the relay wire currently
   returns `validation_namespace_routing_unavailable` (a deliberate stub from
   `rename-request-routing-field`). Bundle-bound agents cannot reach the
   relay-wide operator session (`@GLOBAL`), and the operator cannot be `Cc`'d
   in agent replies. This needs to be fixed before next release.

The correct unified solution is suffix-based target routing: the relay reads
the `@<namespace>` suffix from each target principal ID and routes to the
appropriate registry — no explicit `namespace` needed from the client for
per-target operations.

## What Changes

- **Suffix-based routing in the relay**: for `Send`, the relay reads each
  target's `@<namespace>` suffix. `@GLOBAL` targets route to the relay-wide
  registry; `@<bundle>` targets route to that bundle's registry; bare targets
  (no suffix) default to the sender's bound bundle, or error if the sender is
  relay-wide.
- **Remove `namespace` from MCP `send`, `look`, and `raww`**: these tools do
  not need a namespace parameter; routing is inferred from target principal IDs.
- **GLOBAL delivery implemented**: the relay-wide registry lookup path for
  `@GLOBAL` targets is built; `validation_namespace_routing_unavailable` is
  retired.
- **`List` with `namespace`**: the explicit `namespace` selector on `List` is
  kept and extended — `namespace = "GLOBAL"` returns registered relay-wide
  sessions (resolves `todos/relay/61`, unblocks `todos/mcp/30`).
- **Wire `namespace` field on Send/Look/Raww**: clients should omit it; relay
  ignores it for these operations in favour of per-target suffix inference.
  The field remains on the wire envelope for `List` as the registry selector.

## Impact

- Affected specs: `session-relay`, `mcp-tool-surface`
- Affected code: `src/relay/connection.rs`, `src/relay/handlers.rs`,
  `src/relay/stream.rs`, `src/mcp/params.rs`, `src/mcp/server.rs`,
  `src/mcp/help.rs`, `src/mcp/validation.rs`
- Closes: `todos/relay/61`; unblocks `todos/mcp/30`
- Retires: `validation_namespace_routing_unavailable`
- Depends on: `rename-request-routing-field` (merged)
