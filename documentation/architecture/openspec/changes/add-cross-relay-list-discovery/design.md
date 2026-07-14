# Design: Cross-relay list discovery

## Context

The shipped cross-relay routing model defines outbound `[[peers]]` entries with
`alias`, `address`, and `connect-as`. Connections are lazy, present the
peer-issued `<connect-as>@RELAY` identity, and use the peer PSK at
`<state-root>/peers/<alias>.psk`. On the receiving relay, the authenticated peer
principal's registered `scope` bounds ingress reach.

Discovery has three independent questions: which relays are configured, which
namespaces one relay exposes, and which principals one namespace contains. The
interface keeps those resources separate instead of encoding relay selection in
a special principal-list command.

## Goals

- Give MCP clients schema-level enum hints for meta-tool command selectors.
- Discover locally configured relay aliases without opening connections.
- Discover local or foreign namespaces and principals through one orthogonal
  `relay` selector.
- Reuse existing origin list policy and receiving peer ingress scope.
- Avoid leaking foreign namespaces or principals outside the receiving relay's
  granted scope.

## Non-Goals

- Cross-relay `Look` or snapshots.
- Multi-hop or transitive relay topology discovery.
- Declarative inbound scope configuration.
- A new per-operation peer ACL. The shipped peer `scope` applies uniformly to
  ingress target operations; independently permitting `Send` while denying
  `List` requires a future capability-specific peer policy model.

## Decisions

### D1 — Constrain all MCP meta-tool command selectors now

`list.command` is the immediate driver, but `updown.command`, `new.command`, and
`change.command` have the same plain-string schema failure mode. Small serde and
schemars enums SHALL make their values visible in MCP input schemas while
preserving the existing invalid-params taxonomy.

The `list.command` enum becomes `principals`, `namespaces`, `relays`, and
`decisions`.

### D2 — Keep resource and relay selection orthogonal

The three discovery shapes are:

```json
{"command":"relays","args":{}}
```

```json
{"command":"namespaces","args":{"relay":"west"}}
```

```json
{
  "command":"principals",
  "args":{"relay":"west","namespace":"myapp"}
}
```

`relay` is always the origin relay's local `[[peers]].alias`. It is omitted for
local discovery. A peer never interprets the origin's alias and a forwarded
discovery request carries no onward relay selector.

### D3 — `list.relays` enumerates the local outbound routing table

`list.relays` returns the configured aliases accepted by cross-relay addressing
and by the `relay` argument:

```json
{
  "schema_version":"<SCHEMA_VERSION>",
  "relays":[{"alias":"west"}]
}
```

Only the alias is returned. Socket addresses, `connect-as` identities, credential
paths, and credentials are not discovery output. Entries are sorted by alias.
Listing is configuration-only and SHALL NOT eagerly dial peers or change the
lazy connection lifecycle. The relay reads aliases directly from the normalized
`RelayRuntimeConfiguration.peers` entries; it does not ask
`PeerConnectionManager` to enumerate or connect.

### D4 — `list.namespaces` lists locally visible or peer-visible namespaces

With no `relay`, `list.namespaces` derives namespaces from the local bundle
catalog and relay-wide `GLOBAL` view, then applies the same visibility as local
`list.principals`: a bundle-bound requester sees its home namespace and `GLOBAL`,
while `list` scope `all` permits all configured bundles plus `GLOBAL`. With
`relay`, the origin requires `list` reach at the `all` tier and forwards
discovery to the selected peer.

The response contains sorted namespace identifiers:

```json
{
  "schema_version":"<SCHEMA_VERSION>",
  "relay":"west",
  "namespaces":["myapp"]
}
```

`relay` is omitted for local discovery. A receiving relay derives namespaces
only from its own bundle catalog and `GLOBAL` registry; it does not accept names
asserted by the origin. It returns only namespaces containing at least one
principal covered by the peer's ingress scope. An absent ingress scope rejects
discovery with `authorization_forbidden`.

A namespace-scoped grant does not make an empty namespace discoverable. If the
namespace has no configured or registered principals, namespace discovery omits
it and therefore does not reveal whether the empty namespace exists.

### D5 — `list.principals` gains an optional relay selector

With no `relay`, `list.principals` retains current behavior: omitted namespace
selects the associated home bundle, a bundle name selects that bundle, `GLOBAL`
selects relay-wide principals, and `*` performs local adapter-owned fan-out.

With `relay`, `namespace` is required and SHALL be one concrete namespace. `*`
and reserved unsupported namespace tokens are rejected. Empty/whitespace
`namespace` and empty/whitespace or `*` relay selectors are also rejected before
relay submission. Requiring callers to use `list.namespaces` first avoids an
overloaded foreign all-mode and keeps foreign principal responses bounded.

The origin returns the canonical list aggregation with its local relay selector:

```json
{
  "schema_version":"<SCHEMA_VERSION>",
  "relay":"west",
  "bundles":[]
}
```

Each `ListedBundle.id` remains the foreign namespace authored by the peer. The
origin does not synthesize or rewrite peer results. A principal-scoped ingress
grant may expose only a subset of a namespace; such a bundle carries
`principals_partial=true`. Complete listings omit the field (`None`, never
`Some(false)`).

### D6 — Foreign discovery uses both trust domains

The origin relay authorizes `list.namespaces` and foreign `list.principals`
against the requester's local `list` control at the `all` tier before contacting
the peer.

`list.relays` also requires local `list` scope `all`, because configured peer
aliases are relay-wide cross-boundary routing information.

The receiving relay authenticates the caller as `<id>@RELAY` and applies the
same registered ingress `scope` used by other cross-relay verbs:

- namespace scope exposes that namespace and all of its principals;
- exact principal scope exposes only that principal and its namespace;
- absent scope rejects discovery with `authorization_forbidden`;
- a concrete namespace outside the scope is rejected with
  `authorization_forbidden` without revealing whether it exists.

The current peer scope is target-based rather than capability-specific. Thus a
peer allowed to reach a target may discover it through `List`; there is no
independent `list` bit in this change. A future peer policy model can add
per-operation controls if operators need to permit delivery while denying
discovery.

Receiving-side projection is a thin adapter over the shipped
`scope_permits(scope, target_principal_id)` semantics in
`src/relay/identity.rs`: exact principal match, namespace match, and fail-closed
`None`. It reuses `RouteAuthorization::Ingress`; it does not introduce a new
authorization family.

Forwarded namespace/principal discovery carries no `on_behalf_of`: there is no
delivered attribution envelope, and the receiving relay authorizes the
authenticated peer principal only.

### D7 — Reuse peer connection and error classifications

Foreign discovery uses `PeerConnectionManager` and the existing lazy connection,
credential, and Hello model. Unknown aliases return `validation_unknown_peer`.
Missing credentials return `runtime_peer_credential_missing`; dial or
authentication failure returns `runtime_peer_unavailable`, distinct from local
`relay_unavailable`. Discovery propagates peer errors directly rather than
folding them into a `SendOutcome`.

## Risks / Trade-offs

- The current single-entry peer scope means foreign namespace discovery usually
  returns at most one namespace. This is faithful to the shipped authorization
  model; richer multi-scope grants are separate work.
- Principal listing can reveal names within the peer-granted scope. That is the
  purpose of discovery and is bounded by both origin `list` authorization and
  receiving ingress scope.
- A uniform `authorization_forbidden` for nonexistent and out-of-scope concrete
  namespaces confirms that the peer relay is reachable but reveals no namespace
  existence. Peer reachability is already observable through the authenticated
  connection and is acceptable in this threat model.
