# Design

## Namespace token semantics

A single `namespace` argument replaces `{ all, bundle_name }`:

| `namespace` value | Scope |
|-------------------|-------|
| omitted / `null`  | associated/home bundle (default) |
| `"<bundle>"`      | that configured bundle |
| `"GLOBAL"`        | relay-wide principals |
| `"*"`             | adapter-owned fan-out across all namespaces |

`"*"` is the only fan-out spelling. No `"ALL"` alias is accepted — fewer
equivalent spellings to validate keeps the surface alpha-clean. `"EXTERNAL"` /
`"RELAY"` remain reserved relay-internal namespaces and are not valid `list`
selectors (consistent with the wire-selector rules `add-relay-routing-layer`
formalizes).

## Where the fan-out lives

`"*"` fan-out is **adapter-owned** (MCP server / CLI), identical in mechanism to
today's `all = true`: query each bundle in lexicographic id order, fail fast on
the first `authorization_forbidden`. The relay continues to accept only a single
resolved namespace (a bundle name or `GLOBAL`); it never receives `"*"`. This
keeps the relay contract unchanged on the routing axis and confines the reshape
to the parameter surface.

## Collection rename vs entry type

The rename is the **collection key** only: `ListedBundle.sessions` →
`ListedBundle.principals`. The entry type `ListedSession` and its fields
(`id`, `name?`, `transport`, `ready`) are deliberately untouched:

- Within a bundle, members are sessions; the per-entry vocabulary stays
  `session`-shaped.
- The collection name moves to `principals` because relay-wide
  (`namespace="GLOBAL"`) listing surfaces operators / external apps / peer
  relays in the same array, for which `sessions` is a misnomer.

Consequently the per-entry requirements (`Per-Session Readiness In List
Payload`, `Bundle Hosted Flag In List Payload`) need no change — they describe
the entry type and bundle aggregates, not the collection key.

## Command + help vocabulary

`command="sessions"` → `command="principals"` and the help query
`list.sessions` → `list.principals` move together. A half-rename (e.g. a
`principals[]` payload returned under a `sessions` command) is worse than either
extreme, so the subcommand, the help query, and the result key change in one
cut.

## Sequencing rationale

`add-relay-routing-layer` establishes `namespace` as the uniform wire selector
and fixes cross-bundle list authorization (requester always resolved in the
dispatch bundle). Landing this reshape afterward means the new single
`namespace` param targets a real namespace router. Ordering:
`relay/73` routing layer → **this change** → surface-wide
`bundle_name`→`namespace` rename (separate later proposal).
