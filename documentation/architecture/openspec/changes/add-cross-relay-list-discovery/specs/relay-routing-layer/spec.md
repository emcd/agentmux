## ADDED Requirements

### Requirement: Cross-Relay Discovery Origin Authorization

The origin relay SHALL authorize foreign namespace and principal discovery using
the requester's local `list` control before opening or using a peer connection.
Cross-relay discovery SHALL require the `all` scope tier.

Relay alias enumeration is local routing-table discovery and SHALL require the
requester's `list` control at the `all` tier. Local namespace discovery SHALL
mirror local principal visibility: a bundle-bound requester authorized below
`all` sees its home namespace and `GLOBAL`; a requester authorized at `all` sees
all configured bundle namespaces and `GLOBAL`.

#### Scenario: Deny foreign discovery before peer contact

- **WHEN** a requester whose `list` control is narrower than `all` selects a
  foreign relay
- **THEN** the origin returns `authorization_forbidden`
- **AND** does not contact the peer

#### Scenario: Permit foreign discovery under all

- **WHEN** a requester whose `list` control is `all` selects a configured peer
- **THEN** origin authorization permits peer forwarding

#### Scenario: Relay aliases require all scope

- **WHEN** a requester whose `list` control is narrower than `all` invokes
  `list.relays`
- **THEN** the origin returns `authorization_forbidden`

#### Scenario: Local namespace visibility follows list scope

- **WHEN** a bundle-bound requester invokes local namespace discovery under
  `list` scope narrower than `all`
- **THEN** the result contains its home namespace and `GLOBAL`
- **AND** omits peer bundle namespaces

### Requirement: Cross-Relay Discovery Ingress Filtering

The receiving relay SHALL authorize discovery using the authenticated peer relay
principal's registered ingress `scope`, reusing the target coverage semantics of
`RouteAuthorization::Ingress` and `scope_permits`.

The receiving relay SHALL derive results only from its own bundle catalog and
`GLOBAL` registry. It SHALL NOT use a foreign origin principal, `on_behalf_of`,
an origin-supplied catalog, or an origin-local relay alias as authorization or
discovery input.

Ingress behavior SHALL be:

- namespace scope exposes that namespace and all principals in it;
- exact principal scope exposes that principal and its namespace only;
- absent scope rejects namespace and principal discovery with
  `authorization_forbidden`;
- concrete namespace discovery outside the scope returns
  `authorization_forbidden` without revealing whether the namespace exists.

Namespace discovery SHALL filter its result to namespaces containing at least
one scope-covered principal. Principal discovery under an exact-principal scope
SHALL return a subset marked `principals_partial=true` when other configured
principals are omitted. Complete listings SHALL omit the marker.

The shipped peer ingress scope is operation-agnostic. This requirement does not
add a capability-specific `list` permission separate from target scope.

A namespace-scoped grant for a namespace containing no configured or registered
principals SHALL NOT make that namespace discoverable. Namespace discovery SHALL
omit it, producing the same result as an absent namespace.

#### Scenario: Namespace scope exposes complete namespace

- **WHEN** peer scope is namespace `myapp`
- **AND** the peer requests namespaces or principals
- **THEN** namespace discovery may include `myapp`
- **AND** principal discovery for `myapp` returns its complete listing

#### Scenario: Exact principal scope exposes partial namespace

- **WHEN** peer scope is `agent@myapp`
- **THEN** namespace discovery returns only `myapp`
- **AND** principal discovery returns only `agent`
- **AND** marks the bundle `principals_partial=true` when other principals were
  omitted

#### Scenario: Empty namespace under namespace scope is omitted

- **WHEN** peer scope is namespace `myapp`
- **AND** `myapp` contains no configured or registered principals
- **THEN** namespace discovery omits `myapp`
- **AND** does not reveal whether `myapp` exists

#### Scenario: Absent scope denies discovery

- **WHEN** an authenticated peer principal has no registered ingress scope
- **THEN** namespace and principal discovery return `authorization_forbidden`

#### Scenario: Out-of-scope namespace reveals no existence

- **WHEN** a peer requests principals for a namespace outside its scope
- **THEN** the receiving relay returns `authorization_forbidden`
- **AND** the response does not reveal whether the namespace exists
