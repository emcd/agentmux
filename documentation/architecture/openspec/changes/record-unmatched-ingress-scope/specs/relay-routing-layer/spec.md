## MODIFIED Requirements

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

Where an ingress scope covers no namespace on the receiving relay, namespace
discovery SHALL record that scope and the requesting principal in the receiving
relay's own inscriptions. That record SHALL NOT alter the response, which remains
the empty result required above. Non-disclosure governs what the receiving relay
reports to the peer; it does not govern what the relay retains for the operator who
issued the grant.

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

#### Scenario: Scope covering nothing is recorded rather than refused

- **WHEN** an authenticated peer's ingress scope covers no namespace on the
  receiving relay
- **THEN** namespace discovery returns an empty result rather than
  `authorization_forbidden`
- **AND** the response does not reveal whether any namespace exists
- **AND** the receiving relay records the scope and the requesting principal

#### Scenario: Absent scope denies discovery

- **WHEN** an authenticated peer principal has no registered ingress scope
- **THEN** namespace and principal discovery return `authorization_forbidden`

#### Scenario: Out-of-scope namespace reveals no existence

- **WHEN** a peer requests principals for a namespace outside its scope
- **THEN** the receiving relay returns `authorization_forbidden`
- **AND** the response does not reveal whether the namespace exists
