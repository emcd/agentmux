## MODIFIED Requirements

### Requirement: Cross-Relay Sender Attribution Forwarding

The forwarding (origin) relay SHALL stamp the outbound request's `on_behalf_of`
field, when forwarding a `Send` or `Raww` to a peer relay, with the canonical
authenticated identity (`principal_id`) of the originating requester, when that
requester is a verified principal. When the originating requester is
unauthenticated (socket-trust, or otherwise without a verified principal), the
forwarding relay SHALL omit `on_behalf_of` — an unauthenticated origin cannot be
attributed.

The receiving relay SHALL carry a peer-supplied `on_behalf_of` value, without
interpretation, into the delivered `incoming_message` envelope (and into `Send`/
`Look` responses where the sender-attribution schema surfaces it), alongside
`authenticated_identity`, which reflects the authenticated peer relay principal.
The receiving relay SHALL NOT parse, validate, or resolve the value against its
own principal store; the origin principal lives in the peer's namespace.

When `on_behalf_of` is present, the receiving relay SHALL compose the delivered
message's sender identity as `<on_behalf_of>!<peer-name>`, where `<peer-name>` is
this relay's name for the asserting peer per the Peer Naming Authority
requirement. The composed identity SHALL be the sender carried by the
pane-envelope header, the `incoming_message` event's `sender_session`, and the
relay's envelope metadata record, so that all three name the same sender.
Composition SHALL be uniform: the receiving relay SHALL NOT inspect, classify,
normalize, or reject the origin segment on the way in, and SHALL emit the same
form whatever the peer supplied. Composing this identity is not resolution — the
origin segment is copied, never validated against this relay's principal store.

The composed identity is a resolvable reply address **when the origin segment is
a routable canonical principal id**, which is what the stamping obligation above
requires of a conforming forwarding relay. That guarantee is conditional on the
peer's conformance, not on any check by the receiver, which is what keeps it
compatible with carrying the value uninterpreted.

A peer MAY nonetheless supply an origin that is unqualified, or qualified with a
namespace that names no routable recipient. Such a value SHALL still be composed
and displayed, because the provenance it records is accurate regardless of
whether the origin can be addressed. A reply to it SHALL fail at the replying
relay's own target resolution with that stage's structured validation error, and
SHALL NOT be routed. The receiving relay SHALL NOT substitute, repair, or omit a
non-conforming origin: the failure belongs to the peer that asserted it, and
suppressing the identity would discard the only record of who the peer claimed
to be acting for.

This is safe against misdirection rather than merely loud. The peer segment is
derived locally and always names the peer that actually connected, so a reply is
never routed to a different peer; a non-conforming origin fails before routing,
and a well-formed origin that does not exist on the peer fails at the peer as an
unknown target.

Naming both segments is required rather than rendering the origin alone. The
receiving relay authenticates the peer relay, not the foreign origin, so an
identity carrying only the origin would present an advisory peer-supplied claim
with the same authority as a locally verified sender. Naming the asserting peer
alongside it keeps the provenance visible in the identity itself.

When `on_behalf_of` is absent, the receiving relay SHALL attribute the delivered
message to the peer relay principal, qualified exactly once. A principal id that
already carries its namespace suffix SHALL NOT be re-qualified.

`on_behalf_of` is advisory and asserted by the peer relay: the receiving relay
authenticates the peer relay, not the foreign origin principal. It SHALL NOT be
used as an authorization input — the target-side ingress filter continues to gate
solely on the peer relay principal's `scope` (see the `relay-routing-layer`
capability). Per-origin-principal ingress filtering that consumes `on_behalf_of`
is out of scope for this change.

This requirement defines a **single-hop** attribution: the forwarding relay stamps
`on_behalf_of` with the origin requester it directly authenticated, and the
receiving relay reads that value relative to the accompanying
`authenticated_identity` (the asserting intermediary), never as a globally
resolvable principal. Composition of multiple attribution setters in a single
request — for example an extension or application principal that already carries
its own `on_behalf_of` claim and then initiates a cross-relay delivery — is out of
scope: this change does not define precedence or an attribution-chain shape
between setters. A future extension-app delegation proposal SHALL define that
combined case if it is needed.

#### Scenario: Authenticated origin forwarded with on_behalf_of

- **WHEN** a verified session issues a cross-relay `Send` to a `!<alias>` target
- **THEN** the forwarded outbound request carries `on_behalf_of` set to the origin
  session's canonical `principal_id`
- **AND** the receiving relay's delivered envelope carries that `on_behalf_of`
  alongside `authenticated_identity` naming the peer relay principal

#### Scenario: Delivered sender names the origin and its asserting peer

- **WHEN** a peer authenticating as `bravo@RELAY` forwards a `Send` carrying
  `on_behalf_of` set to `coordinator@agentmux`
- **THEN** the delivered sender identity is `coordinator@agentmux!bravo`
- **AND** the pane-envelope `From` header, the `incoming_message` event's
  `sender_session`, and the envelope metadata record all carry that identity

#### Scenario: Compose a relay-wide origin

- **WHEN** a forwarded `Send` carries `on_behalf_of` set to `operator@GLOBAL`
- **THEN** the delivered sender identity carries that origin unchanged ahead of
  the peer name

#### Scenario: Compose an origin that names no routable recipient

- **WHEN** a forwarded `Send` carries `on_behalf_of` set to a value that is
  unqualified, or qualified with a namespace naming no routable recipient
- **THEN** the delivered sender identity still carries that origin unchanged
  ahead of the peer name
- **AND** the receiving relay neither rejects the delivery nor alters the origin

#### Scenario: A reply to a non-routable origin fails before routing

- **WHEN** a recipient replies to a composed sender whose origin segment names no
  routable recipient
- **THEN** the replying relay's target resolution rejects it with a structured
  validation error
- **AND** no request is forwarded to any peer

#### Scenario: Unauthenticated origin omits on_behalf_of

- **WHEN** a socket-trust (unverified) session issues a cross-relay `Send`
- **THEN** the forwarded outbound request omits `on_behalf_of`
- **AND** the delivered envelope attributes only the peer relay principal

#### Scenario: An already-qualified peer principal is not re-qualified

- **WHEN** the delivered sender falls back to a peer relay principal whose id
  already carries its namespace suffix
- **THEN** the delivered sender identity carries that suffix exactly once

#### Scenario: on_behalf_of is not an ingress authorization input

- **WHEN** a peer relay forwards a request carrying an `on_behalf_of` value
- **AND** the target is outside the peer relay principal's registered `scope`
- **THEN** the receiving relay returns `authorization_forbidden`
- **AND** the ingress decision does not consult `on_behalf_of`

## ADDED Requirements

### Requirement: Peer Naming Authority

A relay's local name for a peer SHALL be the bare relay id of the identity that
relay issued that peer via `new peer <id>@RELAY`. The principal store is
therefore the authority for peer naming, and `[[peers]].alias` restates a name
the relay already holds rather than establishing a second one.

A relay SHALL name an authenticated inbound peer by the bare relay id of that
connection's authenticated principal. Because the relay issued that identity, the
name requires no lookup against the outbound routing table and no correspondence
between the two records.

A relay SHALL NOT derive a peer's name from the `connect-as` identity in that
peer's `[[peers]]` entry, nor from any value the peer supplies. Those name the
opposite direction of the relationship: `connect-as` is what the peer issued this
relay, determined by the peer, and peers determine such identities independently.

A peer this relay has issued an identity but holds no `[[peers]]` entry for — a
peer it receives from but never dials — SHALL still be nameable by that identity.
Naming such a peer SHALL NOT imply it is routable: a cross-relay target naming a
peer with no outbound entry SHALL continue to fail as an unknown peer at delivery
time. A relay SHALL NOT synthesize an alias for a peer it has issued no identity.

#### Scenario: Name an inbound peer from its authenticated principal

- **WHEN** a peer relay authenticates inbound as `bravo@RELAY`
- **THEN** the receiving relay's name for that peer is `bravo`
- **AND** deriving it consults no `[[peers]]` entry

#### Scenario: Name a receive-only peer that has no outbound entry

- **WHEN** a peer authenticates inbound as `bravo@RELAY`
- **AND** this relay holds no `[[peers]]` entry naming `bravo`
- **THEN** the receiving relay's name for that peer is still `bravo`

#### Scenario: A nameable peer is not necessarily routable

- **WHEN** a cross-relay target names a peer for which this relay holds no
  `[[peers]]` entry
- **THEN** delivery fails as an unknown peer
- **AND** the failure is not suppressed by that peer being nameable

#### Scenario: Peer naming ignores the presented identity

- **WHEN** a peer's `[[peers]]` entry carries a `connect-as` differing from its
  `alias`
- **THEN** this relay's name for that peer is the `alias`
- **AND** the `connect-as` value is used only to authenticate this relay's
  outbound connection to that peer
