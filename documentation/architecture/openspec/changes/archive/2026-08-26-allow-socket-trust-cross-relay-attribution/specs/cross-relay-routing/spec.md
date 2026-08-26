## MODIFIED Requirements

### Requirement: Cross-Relay Sender Attribution Forwarding

The forwarding (origin) relay SHALL stamp the outbound request's `on_behalf_of`
field, when forwarding a `Send` or `Raww` to a peer relay, with the canonical
`principal_id` of the originating requester — the identity the relay admitted
that requester under, whether that identity was verified against the principal
store or accepted as a socket-trust claim.

Attribution follows admission rather than re-deciding it. A relay that requires
session credentials admits no unverified principal, so every attribution it
forwards is store-backed. A relay that accepts socket-trust has already decided
that a session's claim about itself is good enough to deliver on locally, and
withholding the same claim from a peer would leave its cross-relay attribution
inconsistent with its own delivery for no gain: the claim is already the identity
under which that session sends, receives, and appears to every local recipient.
The `require-session-credentials` setting is where that decision is made, and
SHALL remain the only place it is made — no separate setting SHALL gate
attribution independently of admission.

The relay SHALL NOT populate `authenticated_identity` for an unverified
requester, which the `relay-identity` capability's Sender Attribution Schema
requires to be omitted rather than self-asserted. `on_behalf_of` and
`authenticated_identity` remain separately sourced: the first carries who the
origin was admitted as, the second carries whether a credential backed it.

A requester SHALL NOT supply the attribution the relay forwards. An
`on_behalf_of` value carried on a request from a non-ingress requester SHALL be
discarded, as it is today, and the relay SHALL forward the identity established
at Hello in its place. Admission is per-connection; attribution SHALL NOT become
per-request.

Discarding the value rather than refusing the request is existing behavior this
change preserves deliberately. Attribution now comes from admission, so a
supplied value has nothing to contribute and its absence costs the requester
nothing: the message is delivered correctly attributed rather than refused.
Making the same case an error would be a wire-visible change to the request
contract, which is a separate question from where attribution comes from.

This governs only the value forwarded to a peer. Attribution on locally
delivered messages is unchanged: an `on_behalf_of` arriving through ingress from
a peer is carried into local delivery as before, and a non-ingress requester's
own value is dropped before it reaches a delivered envelope.

The receiving relay SHALL carry a peer-supplied `on_behalf_of` value, without
interpretation, into the delivered `incoming_message` envelope (and into `Send`/
`Look` responses where the sender-attribution schema surfaces it), alongside
`authenticated_identity`, which reflects the authenticated peer relay principal.
The receiving relay SHALL NOT parse, validate, or resolve the value against its
own principal store; the origin principal lives in the peer's namespace. The
receiving relay SHALL NOT distinguish an origin a peer admitted on a socket-trust
claim from one it verified against its store, and no wire field SHALL convey that
distinction. The receiver cannot verify a foreign origin under either, so the
value is advisory in both cases, and a marker would invite treating one as weaker
evidence — an interpretation the receiver is forbidden to make.

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

A peer MAY omit `on_behalf_of` — an implementation predating this requirement, or
one that declines to attribute. When it is absent, the receiving relay SHALL
attribute the delivered message to the peer relay principal, qualified exactly
once. A principal id that already carries its namespace suffix SHALL NOT be
re-qualified. That attribution names no routable recipient and SHALL NOT be made
to resolve as one: it records that the message arrived over an authenticated peer
connection whose origin the receiving relay does not know, and a reply to it
SHALL fail at target resolution like any other unroutable target.

`on_behalf_of` is advisory and asserted by the peer relay: the receiving relay
authenticates the peer relay, not the foreign origin principal. It SHALL NOT be
used as an authorization input — the target-side ingress filter continues to gate
solely on the peer relay principal's `scope` (see the `relay-routing-layer`
capability). Per-origin-principal ingress filtering that consumes `on_behalf_of`
is out of scope for this change.

This requirement defines a **single-hop** attribution: the forwarding relay stamps
`on_behalf_of` with the origin requester it directly admitted, and the receiving
relay reads that value relative to the accompanying `authenticated_identity` (the
asserting intermediary), never as a globally resolvable principal. Composition of
multiple attribution setters in a single request — for example an extension or
application principal that already carries its own `on_behalf_of` claim and then
initiates a cross-relay delivery — is out of scope: this change does not define
precedence or an attribution-chain shape between setters. A future extension-app
delegation proposal SHALL define that combined case if it is needed.

#### Scenario: Authenticated origin forwarded with on_behalf_of

- **WHEN** a verified session issues a cross-relay `Send` to a `!<alias>` target
- **THEN** the forwarded outbound request carries `on_behalf_of` set to the origin
  session's canonical `principal_id`
- **AND** the receiving relay's delivered envelope carries that `on_behalf_of`
  alongside `authenticated_identity` naming the peer relay principal

#### Scenario: Socket-trust origin forwarded with the identity it was admitted under

- **WHEN** a relay accepting socket-trust admits a session claiming
  `coordinator@agentmux`
- **AND** that session issues a cross-relay `Send`
- **THEN** the forwarded outbound request carries `on_behalf_of` set to
  `coordinator@agentmux`
- **AND** the delivered sender identity composes to `coordinator@agentmux!<peer>`
  and resolves as a reply target

#### Scenario: Attribution without a verified identity

- **WHEN** a socket-trust origin is attributed
- **THEN** the delivered envelope carries that `on_behalf_of`
- **AND** `authenticated_identity` for that requester remains absent

#### Scenario: A requester-supplied attribution is discarded, not forwarded

- **WHEN** a session issues a cross-relay `Send` carrying an `on_behalf_of` value
  naming a principal other than the one it was admitted under
- **THEN** the request is not refused on account of that value
- **AND** the forwarded outbound request carries `on_behalf_of` set to the
  identity established at Hello
- **AND** the supplied value appears in neither the forwarded request nor the
  delivered envelope

#### Scenario: Local delivery attribution is unchanged

- **WHEN** a non-ingress requester supplies an `on_behalf_of` value on a local
  `Send`
- **THEN** the delivered envelope carries no `on_behalf_of`

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

#### Scenario: An already-qualified peer principal is not re-qualified

- **WHEN** the delivered sender falls back to a peer relay principal whose id
  already carries its namespace suffix
- **THEN** the delivered sender identity carries that suffix exactly once

#### Scenario: A reply to the unattributed fallback fails at resolution

- **WHEN** a peer omits `on_behalf_of`
- **AND** a recipient replies to the resulting delivered sender, in either the
  plain or the bang-path form
- **THEN** target resolution rejects it with a structured validation error
  reporting that the namespace names no routable recipient
- **AND** no request is forwarded to any peer

#### Scenario: on_behalf_of is not an ingress authorization input

- **WHEN** a peer relay forwards a request carrying an `on_behalf_of` value
- **AND** the target is outside the peer relay principal's registered `scope`
- **THEN** the receiving relay returns `authorization_forbidden`
- **AND** the ingress decision does not consult `on_behalf_of`
