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

Cross-boundary sender attribution is out of scope for this slice: the delivered
`incoming_message` envelope on the receiving relay reflects the peer relay as the
authenticated sender under the existing sender-attribution contract. Carrying the
*original* sender identity across the boundary (the reserved `on_behalf_of`
field) is deferred to a follow-on that specifies that setting mechanism and its
`relay-identity` delta.

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

