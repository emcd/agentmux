# Design: Cross-relay sender attribution via on_behalf_of

## Context

relay/90 delivered outbound cross-relay `Send`/`Raww` forwarding but deliberately
deferred cross-boundary sender attribution (D5). On the receiving relay, a
forwarded delivery is authenticated as the peer relay principal
(`<connect-as>@RELAY`); the delivered `incoming_message` envelope therefore names
the peer relay — not the human/agent principal inside the peer that originated the
message. The `relay-identity` Sender Attribution Schema already reserves an
`on_behalf_of` field for exactly this "authenticated intermediary acting on behalf
of an origin principal" case (`extensions-protocol` framing), but leaves its
setting mechanism unspecified. This change specifies that mechanism for the
cross-relay path.

The trust framing is the same two-domain framing that drove relay/90's ingress
filter: the origin relay and the receiving relay are separate trust domains. The
receiving relay authenticates the *peer relay*, and can therefore trust "this
message came from peer X" — but it cannot independently verify *which* principal
inside X actually sent it. `on_behalf_of` is thus an assertion by X, carried for
attribution/observability, never as an authorization input.

## Goals / Non-Goals

- Goals:
  - Specify what the forwarding relay sets `on_behalf_of` to, and when.
  - Specify that the receiving relay carries it uninterpreted into the delivered
    envelope alongside the peer-relay `authenticated_identity`.
  - Keep authorization unchanged: ingress stays peer-relay-scoped.
- Non-Goals:
  - Per-origin-principal ingress filtering that consumes `on_behalf_of`.
  - The trusted-host-supplied `on_behalf_of` on `IdentityIntrospect` records
    (a distinct setter that stays reserved).
  - Multi-hop attribution chaining (single-peer targets; no re-forward).
  - Extension-app delegation composition — precedence or an attribution-chain
    shape when both the cross-relay setter and a future extension-app
    `on_behalf_of` setter are active in one request (EE review).
  - Cross-relay `List`/`Look` attribution (those operations are not yet
    forwarded — `todos/relay/100`).

## Decisions

### D1 — The forwarding relay sets on_behalf_of to the origin principal_id

When the origin requester is a verified principal, the origin relay stamps
`on_behalf_of` on the outbound forwarded request with that requester's canonical
`principal_id` (the same value it would place in `authenticated_identity` for a
local delivery). This reuses the identity the origin relay already authenticated;
no new lookup.

### D2 — Unauthenticated origin omits the field

A socket-trust / unverified origin has no `principal_id`; there is nothing to
attribute. The origin relay omits `on_behalf_of` rather than asserting a
self-declared or placeholder value — consistent with the existing rule that
unauthenticated senders omit `authenticated_identity` rather than populate it.

### D3 — The receiving relay carries it uninterpreted

The receiving relay treats an inbound `on_behalf_of` as an opaque string. It does
not parse, validate, or resolve it against its own principal store (the origin
principal lives in the peer's namespace, which may collide with a local one). It
carries the value verbatim into the delivered `incoming_message` envelope, next to
`authenticated_identity = <peer>@RELAY`. The pair fully attributes: "peer relay X,
on behalf of principal Y in X's domain."

### D4 — on_behalf_of is advisory, never an authorization input

The ingress filter (relay-routing-layer) continues to authorize forwarded targets
against the peer relay principal's registered `scope` only. `on_behalf_of` is not
consulted. A future per-origin-principal ingress grammar (deferred) would be the
place that starts *reading* it, and would need its own trust analysis before doing
so.

## Risks / Trade-offs

- **A malicious or buggy peer can assert any `on_behalf_of`.** → Mitigation: the
  field is advisory and explicitly not an authorization input; recipients must
  treat it as "peer-asserted origin," not verified identity. Authorization remains
  anchored on the authenticated peer relay principal and its scope. Documented in
  the spec so consumers do not gate on it.
- **Namespace collision** between an asserted origin id and a local principal id.
  → Mitigation: the value is opaque and never resolved locally; it is
  attribution/display metadata, so a collision has no authorization consequence.

## Resolved during review (EE)

- **Qualification of `on_behalf_of` → RESOLVED (keep the pair).** Do not invent a
  qualified foreign-principal syntax in this proposal. The
  `(authenticated_identity, on_behalf_of)` pair is sufficient: consumers read
  `on_behalf_of` relative to the authenticated intermediary, not as a globally
  resolvable local principal. Reflected in both spec deltas.
- **Single-hop only → RESOLVED (call it out).** This change defines a single-hop,
  intermediary-asserted attribution. Composition of multiple `on_behalf_of`
  setters in one request (e.g. an extension/app principal carrying its own claim
  that then initiates a cross-relay delivery) is out of scope; precedence /
  attribution-chain shape is deferred to a future extension-app delegation
  proposal. Reflected in the `cross-relay-routing` delta and Non-Goals.
- **Extensions-protocol compatibility → CONFIRMED (EE).** No compatibility
  blocker: the cross-relay use and future external-app federation coexist under
  the shared "intermediary-asserted origin subject" semantics.

## Open Questions

- Do we surface `on_behalf_of` on cross-relay `Send` *responses* now, or only on
  the delivered `incoming_message` envelope, given `List`/`Look` forwarding is not
  yet built? (Leaning: envelope now; response surfacing follows the existing
  schema, no extra work.)
