# Change: Collapse list selectors to a single namespace and rename to principals

## Why

The `list` tool encodes one question — *which namespace?* — across two
selectors: `all: bool` (relay-wide fan-out) and `bundle_name: Option<String>`
(one bundle). The `todos/mcp/38` MCP surface audit (`reviews/mcp/1`) confirmed
this two-field/one-concept split and that `namespace` is already the relay's
wire routing selector (a bundle name, or the relay-wide specifiers `GLOBAL` /
`EXTERNAL` / `RELAY`), now being formalized as the dispatch spine by
`add-relay-routing-layer`.

Separately, the result collection key `sessions[]` (on the canonical
`ListedBundle` payload) undercounts what `list` surfaces: when a relay-wide
namespace is listed, operators, external apps, and peer relays appear alongside
bundle members. `principals` is the accurate umbrella term the relay already
uses internally for that set.

## What Changes

- **Collapse the `list` selectors** `{ all, bundle_name }` into a single
  `namespace: Option<String>` argument:
  - omitted / null → associated/home bundle (current default behaviour)
  - `"<bundle>"` → that specific bundle
  - `"GLOBAL"` → relay-wide principals
  - `"*"` → adapter-owned fan-out across all namespaces (current `all = true`)
  - `"*"` is the single canonical fan-out token; there is **no** `"ALL"` alias.
- **Rename the result collection key** `sessions[]` → `principals[]` on the
  canonical `ListedBundle` payload (the serde key follows the Rust field). The
  per-entry type (`ListedSession`) and its fields (`id`, `name?`, `transport`,
  `ready`) are unchanged — only the collection name moves.
- **Rename the subcommand selector** `command = "sessions"` →
  `command = "principals"`, and the help query `list.sessions` →
  `list.principals`, so the tool/command vocabulary is internally consistent
  (one clean cut rather than a half-rename).
- `namespace` semantics are defined to match the wire selector that
  `add-relay-routing-layer` formalizes — **no new routing or authorization code
  here**. This is a surface/param reshape that maps onto the existing/forthcoming
  namespace router. Adapter-owned `"*"` fan-out is unchanged from today's
  `all = true` behaviour; the relay still accepts only a single resolved
  namespace (a bundle name or `GLOBAL`) and never receives `"*"`.

## Impact

- **Affected specs:** `mcp-tool-surface` (List Sessions Selectors, All-Mode
  Aggregation, Unreachable Relay Fallback, Recipient Listing Contract, Help
  Tool), `cli-surface` (List Sessions Command Surface, Machine Output Contract,
  Fanout Behavior, Unreachable Relay Fallback), `session-relay` (Relay List
  Authorization payload-key reference).
- **Affected code:** `src/mcp/params.rs` (`ListArgs`, `ListParams`),
  `src/mcp/server.rs` (list dispatch, namespace forwarding, response map),
  `src/relay/contract.rs` (`ListedBundle.sessions` → `principals`),
  `src/commands/*` (CLI `list` flags + machine output key), `src/mcp/help.rs`
  and help fixtures, any `src/tui/**` consumer of the `sessions` key.
- **BREAKING (alpha, intended):** the `all` / `bundle_name` args, the
  `--all` / `--bundle` CLI flags, the `sessions[]` result key, and the
  `command="sessions"` selector are removed outright — no compatibility shim and
  no negative-assertion logic (per alpha defaults). Clients migrate to
  `namespace` / `principals`.

## Sequencing

This change lands **after** `add-relay-routing-layer` (`todos/relay/73`), which
formalizes `namespace` as the wire selector and enables cross-bundle list, so
the new single `namespace` param maps onto a real namespace router rather than
today's `all` / `bundle_name` forwarding. The broader surface-wide
`bundle_name` → `namespace` response-key rename (audit finding §3) is a separate
later proposal, not part of this one.

## Non-goals

- No routing or authorization changes (owned by `add-relay-routing-layer`).
- No surface-wide `bundle_name` → `namespace` response-key rename across
  `look` / `send` / `raww` (separate follow-up).
- No change to what a principal *is*, to the `ListedSession` entry shape, or to
  how `GLOBAL` / `EXTERNAL` / `RELAY` resolve.

## Provenance

Promotes `ideas/mcp/2`; resolves audit finding §3 from `todos/mcp/38`
(`reviews/mcp/1`). Coordinator-approved direction on all four draft questions
(`ideas/mcp/3`): include the command rename, single `"*"` token, list-scoped,
sequenced after the routing layer.
