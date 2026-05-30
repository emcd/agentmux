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

**D4 — One namespace per request (this slice).**
A single request carries at most one namespace context; all targets in that
request are resolved against the registry for that namespace. Cross-namespace
fan-out — addressing targets in multiple registries in one request (e.g.,
`agent@bundle-a` and `operator@GLOBAL` simultaneously) — is deferred to a
separate `cross-namespace-routing` proposal. The `namespace` field design does
not preclude per-target derivation from the `@<namespace>` suffix in a future
slice. See `designs/relay` nb note for the draft proposal.

**D5 — Broadcast under GLOBAL namespace is out of scope.**
`broadcast = true` with `namespace = "GLOBAL"` (fan-out to all registered
`@GLOBAL` sessions) is not defined in this slice. Relay-wide broadcast
semantics require separate design consideration, including multi-operator
scenarios.

**D6 — EXTERNAL and RELAY namespace specifiers: reserved, not client-routable.**
The relay accepts and parses `EXTERNAL` and `RELAY` as syntactically valid
namespace values on the request envelope. If a client attempts to route directly
to these namespaces, relay returns `validation_unsupported_namespace`. Only the
relay itself routes to these namespaces under defined protocol circumstances
(extension protocol handling, peer-relay forwarding).

## Resolved Questions

- **P.1 Mixed targets**: one namespace per request in this slice; cross-namespace
  fan-out deferred to `cross-namespace-routing` proposal. (→ D4)
- **P.2 Broadcast under GLOBAL**: out of scope; relay-wide broadcast semantics
  filed for separate design review. (→ D5)
- **P.3 Error code**: `validation_missing_target_bundle` renamed to
  `validation_missing_routing_namespace`. (→ task 1.5)
- **P.4 EXTERNAL/RELAY routing**: accepted/parsed but client routing rejected
  with `validation_unsupported_namespace`; only relay routes to these. (→ D6)

## Risks / Trade-offs

- Wire-format breaking change: all existing MCP, TUI, and relay clients that
  pass a bundle routing context must update the field name. Manageable since
  this is alpha software. Coordinate across mcp, tui, relay lanes.
- Expanding `namespace` semantics means relay routing has two code paths
  (catalog lookup vs relay-wide registry). Keep the branching explicit and
  co-located in `resolve_effective_bundle` (or its successor).
