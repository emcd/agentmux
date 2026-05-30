## Context

After identity federation (D1d), the relay request envelope carries an
optional `bundle_name` field that selects the routing context for the request.
Session principals are bundle-bound at Hello; relay-wide principals (`@GLOBAL`,
`@EXTERNAL`, `@RELAY`) are not. The current field name is misleading — not all
routing targets are bundles. This proposal renames the field and expands its
semantics to include relay-wide namespace specifiers.

## Goals / Non-Goals

Goals:
- Rename `bundle_name` → `namespace` on the request envelope wire format.
- Allow a session principal in bundle `A` to address a relay-wide `@GLOBAL`
  target by specifying a namespace value in the request.
- Keep existing bundle-routing behavior unchanged for session principals.

Non-Goals:
- Cross-bundle session-to-session routing (out of scope; separate proposal).
- Discovery of available namespaces (out of scope).

## Decisions

**D1 — Field name: `namespace`.**
The new field name `namespace` reflects that the value selects a routing
context (either a bundle by name, or a relay-wide namespace specifier). This
matches the `principal_id` namespace vocabulary from identity federation.

**D2 — Accepted values.**
- Bundle name (e.g., `"agentmux"`) → catalog lookup; relay routes to that
  bundle. Unchanged from current `bundle_name` behavior.
- Relay-wide specifier (`"GLOBAL"`, `"EXTERNAL"`, `"RELAY"`) → relay routes in
  the relay-wide namespace; target resolution uses the relay-wide registry,
  not the bundle catalog.
- Absent + connection is bundle-bound → route to bound bundle (unchanged).
- Absent + connection is relay-wide + no explicit namespace → typed error.

**D3 — Target resolution with relay-wide namespace.**
When `namespace = "GLOBAL"`, relay resolves target session IDs against the
relay-wide registry (registered `@GLOBAL` connections) rather than bundle
members. `send` to `namespace = "GLOBAL"` with `targets = ["operator@GLOBAL"]`
delivers to the registered relay-wide UI session for that principal_id.

## Open Questions

- **Mixed targets**: can a single `send` request address both bundle-scoped
  sessions (namespace = a bundle name) and relay-wide sessions (namespace =
  "GLOBAL") simultaneously? Current leaning: no — one namespace per request;
  callers issue separate sends if needed. Confirm with BE before implementation.

- **Broadcast semantics under GLOBAL namespace**: if `broadcast = true` and
  `namespace = "GLOBAL"`, should relay broadcast to all registered `@GLOBAL`
  UI sessions? Or is broadcast always within-bundle only? Likely relay-wide
  broadcast is out of scope for this change; confirm.

- **Error code rename**: `validation_missing_target_bundle` was coined for D1d.
  With the rename should it become `validation_missing_routing_namespace`?
  Low-stakes but should be consistent. Confirm preferred name.

- **`@EXTERNAL` and `@RELAY` namespace routing**: the proposal allows these
  specifiers in principle, but the motivating use case is only `@GLOBAL`.
  Should `EXTERNAL` and `RELAY` be reserved but unimplemented in this slice?
  Confirm scope boundary with BE.

## Risks / Trade-offs

- Wire-format breaking change: all existing MCP, TUI, and relay clients that
  pass a bundle routing context must update the field name. Manageable since
  this is alpha software. Coordinate across mcp, tui, relay lanes.
- Expanding `namespace` semantics means relay routing has two code paths
  (catalog lookup vs relay-wide registry). Keep the branching explicit and
  co-located in `resolve_effective_bundle` (or its successor).
