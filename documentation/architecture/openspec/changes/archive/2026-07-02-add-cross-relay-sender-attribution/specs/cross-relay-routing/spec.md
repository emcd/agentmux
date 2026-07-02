## ADDED Requirements

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

#### Scenario: Unauthenticated origin omits on_behalf_of

- **WHEN** a socket-trust (unverified) session issues a cross-relay `Send`
- **THEN** the forwarded outbound request omits `on_behalf_of`
- **AND** the delivered envelope attributes only the peer relay principal

#### Scenario: on_behalf_of is not an ingress authorization input

- **WHEN** a peer relay forwards a request carrying an `on_behalf_of` value
- **AND** the target is outside the peer relay principal's registered `scope`
- **THEN** the receiving relay returns `authorization_forbidden`
- **AND** the ingress decision does not consult `on_behalf_of`

## MODIFIED Requirements

### Requirement: Cross-Relay Delivery Outcome Propagation

A cross-relay `Send` or `Raww` SHALL propagate a delivery outcome back to the
originating requester that reflects the peer relay's response. The outbound
request SHALL carry the origin `request_id`, and the peer's typed response SHALL
map onto the originating requester's delivery-outcome channel so that the
requester can distinguish:

- **delivered** — the peer accepted and delivered to the foreign principal;
- **rejected** — the peer returned a typed rejection (e.g.
  `authorization_forbidden` from the peer's ingress filter, or
  `validation_unknown_target`), carried through with its code;
- **peer_unavailable** — the peer connection could not be established or the
  request could not be sent, distinct from a local `relay_unavailable`.

Cross-boundary sender attribution is specified by the Cross-Relay Sender
Attribution Forwarding requirement: the delivered `incoming_message` envelope on
the receiving relay reflects the peer relay as the authenticated sender and, when
the origin sender was a verified principal, additionally carries the origin
identity in the `on_behalf_of` field.

#### Scenario: Delivered outcome propagated from peer

- **WHEN** a peer relay accepts and delivers a forwarded `Send`
- **THEN** the originating requester observes a delivered outcome keyed to its
  original `request_id`

#### Scenario: Peer ingress rejection propagated as rejected

- **WHEN** a peer relay's ingress filter denies a forwarded target with
  `authorization_forbidden`
- **THEN** the originating requester observes a rejected outcome carrying that
  code, distinct from a peer-unavailable outcome

#### Scenario: Peer-unavailable distinct from relay-unavailable

- **WHEN** the outbound peer connection cannot be established for a cross-relay
  delivery
- **THEN** the originating requester observes a `peer_unavailable` outcome
- **AND** it is distinguishable from a local `relay_unavailable` transport failure
