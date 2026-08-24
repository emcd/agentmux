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
