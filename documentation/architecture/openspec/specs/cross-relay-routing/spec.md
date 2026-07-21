# cross-relay-routing Specification

## Purpose
TBD - created by archiving change add-outbound-peer-relay-routing. Update Purpose after archive.
## Requirements
### Requirement: Outbound Peer Connection Management

The relay SHALL establish and maintain an outbound connection to a configured
peer relay for cross-relay delivery. Connections SHALL be established **lazily**
on the first cross-relay delivery to a given peer, never eagerly at startup, so
that an unreachable peer neither blocks nor destabilizes relay startup.

To connect to a peer, the relay SHALL read the outbound PSK from the well-known
peer credential path `<state-root>/peers/<alias>.psk` (where `<alias>` is the
peer's local `alias` from its `[[peers]]` entry), dial the peer's configured
`address` (a Unix domain socket path in this slice), and present a Hello frame as
the per-peer `<connect-as>@RELAY` principal that peer issued this relay (see the
`runtime-bootstrap` capability's Relay Cross-Relay Presented Identity requirement)
with that PSK. A missing or unreadable credential file SHALL fail the affected
delivery with a typed outcome — not relay startup.

While a peer connection is unavailable (unreachable endpoint or failed
authentication) the relay SHALL surface a typed `peer_unavailable` outcome for
deliveries targeting that peer and SHALL retry establishment with jittered
exponential backoff rather than blocking.

#### Scenario: Lazy connection on first cross-relay delivery

- **WHEN** the relay has a configured peer but no cross-relay delivery has been
  addressed to it
- **THEN** the relay holds no outbound connection to that peer

- **WHEN** the relay routes the first `Send` to a `!<alias>` target for that peer
- **THEN** the relay establishes the outbound connection, presenting the per-peer
  `<connect-as>@RELAY` principal that peer issued this relay, with the peer
  credential

#### Scenario: Unreachable peer does not block startup

- **WHEN** a configured peer's `address` is unreachable at relay startup
- **THEN** relay startup completes normally
- **AND** a subsequent cross-relay delivery to that peer yields a
  `peer_unavailable` outcome

#### Scenario: Missing peer credential fails the delivery only

- **WHEN** a cross-relay delivery is routed to a peer whose
  `<state-root>/peers/<alias>.psk` file is absent or unreadable
- **THEN** the delivery fails with a typed outcome identifying the credential
  problem
- **AND** relay startup and unrelated deliveries are unaffected

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

### Requirement: Cross-Relay Discovery Forwarding

The origin relay SHALL forward namespace and principal discovery to a configured
outbound peer selected by local `alias`. It SHALL reuse the existing lazy peer
connection manager, credential path, and per-peer presented identity used by
cross-relay `Send` and `Raww`.

Forwarded requests SHALL NOT carry the origin-local alias, an onward relay
selector, or `on_behalf_of`. The peer SHALL author all returned namespace and
principal data from its local state. The origin SHALL NOT synthesize or rewrite
foreign namespaces, bundle ids, or principals.

#### Scenario: Forward namespace discovery

- **WHEN** the origin handles namespace discovery for alias `west`
- **THEN** it dials or reuses the configured peer connection for `west`
- **AND** forwards a discovery request with no onward relay selector

#### Scenario: Forward principal discovery

- **WHEN** the origin handles principal discovery for `myapp` on alias `west`
- **THEN** the peer request names namespace `myapp`
- **AND** carries no `on_behalf_of`

#### Scenario: Forwarded discovery omits origin-only selectors

- **WHEN** the origin forwards namespace or principal discovery to a peer
- **THEN** the wire request contains no origin-local relay alias or onward relay
  selector
- **AND** contains no `on_behalf_of`

#### Scenario: Unknown peer alias fails on origin

- **WHEN** discovery names an alias absent from local `[[peers]]`
- **THEN** the origin returns `validation_unknown_peer`
- **AND** does not attempt an outbound connection

### Requirement: Cross-Relay Discovery Outcome Propagation

Cross-relay discovery SHALL propagate peer-authored successes and typed errors
to the origin requester. Foreign namespace responses SHALL contain only
peer-authored namespace ids. Foreign principal responses SHALL contain canonical
`ListedBundle` entries with foreign ids and any `principals_partial` marker
unchanged.

Discovery SHALL reuse peer connection errors:

- `runtime_peer_credential_missing` for absent or unreadable credentials
- `runtime_peer_unavailable` for dial or authentication failure

These errors SHALL remain distinct from local `relay_unavailable`. Unlike
`Send`, discovery SHALL propagate typed relay errors directly rather than folding
them into `SendOutcome`.

#### Scenario: Propagate peer success

- **WHEN** the peer accepts a discovery request
- **THEN** the origin returns the peer-authored result without local synthesis

#### Scenario: Propagate peer authorization denial

- **WHEN** the peer rejects discovery with `authorization_forbidden`
- **THEN** the origin requester receives that typed denial unchanged

#### Scenario: Peer unavailable remains distinct

- **WHEN** the peer connection cannot be established
- **THEN** the origin returns `runtime_peer_unavailable`
- **AND** does not report local `relay_unavailable`

