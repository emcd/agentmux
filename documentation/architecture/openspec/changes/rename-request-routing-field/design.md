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

**D4 — Rename scope: envelope routing selector only.**
The rename targets the routing context selector on `IncomingEnvelope` and the
corresponding `client.rs` request frame field. Per-variant `bundle_name` fields
inside `RelayRequest` variants (e.g., `PermissionList`, `IdentityIntrospect`)
are NOT renamed in this proposal: they identify a bundle-scoped target, not a
routing context specifier. Renaming them to `namespace` would be misleading.
A separate vocabulary cleanup pass can evaluate those fields. This keeps the
vocabulary consistent: `namespace` = routing context selector; `bundle_name`
inside a variant = target identifier within that request.

**D5 — One namespace per request (this slice).**
A single request carries at most one namespace context; all targets in that
request are resolved against the registry for that namespace. Cross-namespace
fan-out — addressing targets in multiple registries in one request (e.g.,
`agent@bundle-a` and `operator@GLOBAL` simultaneously) — is deferred to a
separate `cross-namespace-routing` proposal. The `namespace` field design does
not preclude per-target derivation from the `@<namespace>` suffix in a future
slice. See `designs/relay` nb note for the draft proposal.

**D6 — Broadcast under GLOBAL namespace is out of scope.**
`broadcast = true` with `namespace = "GLOBAL"` (fan-out to all registered
`@GLOBAL` sessions) is not defined in this slice. Relay-wide broadcast
semantics require separate design consideration, including multi-operator
scenarios.

**D7 — EXTERNAL and RELAY namespace specifiers: reserved, not client-routable.**
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
  with `validation_unsupported_namespace`; only relay routes to these. (→ D7)

## Risks / Trade-offs

- Wire-format breaking change: all existing MCP, TUI, and relay clients that
  pass a bundle routing context must update the field name. Manageable since
  this is alpha software. Coordinate across mcp, tui, relay lanes.
- Expanding `namespace` semantics means relay routing has two code paths
  (catalog lookup vs relay-wide registry). Keep the branching explicit and
  co-located in `resolve_effective_bundle` (or its successor).

## Design Correction (post-implementation)

During implementation, MCP tasks 2.1–2.2 propagated `namespace` as an explicit
parameter on `look`, `raww`, and `send` MCP tool schemas. This was a design
error: per-target operations do not need an explicit `namespace` parameter
because the routing context is fully determined by the `@<namespace>` suffix on
each target principal ID. An additional `namespace` field creates two competing
routing mechanisms and forces callers to redundantly state information already
present in their targets.

`namespace` is appropriate only on operations with no target principal IDs
(e.g., `List`), where there is no principal ID suffix to infer the routing
context from.

This correction also supersedes D2's `namespace = "GLOBAL"` routing path for
Send: clients do not specify `namespace = "GLOBAL"`; instead, the relay infers
GLOBAL routing when a target carries the `@GLOBAL` suffix.

The correction is tracked in `add-global-namespace-routing`, which:
- Removes `namespace` from `look`, `raww`, and `send` MCP tool schemas.
- Implements suffix-based routing inference in the relay: when a target carries
  `@GLOBAL`, the relay routes to the relay-wide registry without requiring an
  explicit `namespace = "GLOBAL"` selector from the client.
- Retires `validation_namespace_routing_unavailable`.
- Keeps `namespace` on `List` as the explicit registry selector.
