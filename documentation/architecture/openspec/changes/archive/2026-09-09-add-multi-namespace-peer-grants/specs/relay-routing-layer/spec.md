## MODIFIED Requirements

### Requirement: Cross-Relay Target Ingress Filter

The authorization stage SHALL apply a target-side ingress filter to delivery
from an authenticated peer relay (`<id>@RELAY`). The receiving relay SHALL use
that peer's current registered scope rather than a receiving-bundle policy
for a foreign sender. Origin-side uniform scope-tier authorization SHALL remain
independently required on the forwarding relay.

The filter SHALL authorize every resolved target against the current peer
record using `Peer Ingress Scope Grammar` and `Authoritative Peer Scope Updates`
in the `relay-identity` capability:

- A comma-separated namespace set SHALL cover all addressable targets in those
  namespaces only.
- `*` SHALL cover every namespace with addressable principals, including
  GLOBAL, future bundles and newly introduced addressable namespace types
  immediately.
- Empty or absent scope SHALL cover nothing. An out-of-scope target SHALL be
  rejected with `authorization_forbidden` carrying an ingress-denied detail.
- Every resolved target SHALL be covered before any target in an ingress
  operation is admitted. Authorization and admission SHALL be ordered together
  against scope updates; stale Hello or preparation snapshots SHALL NOT permit
  admission after the grant that authorized them has been replaced.

The ingress filter SHALL be evaluated at the shared `authorize_route` stage,
not in individual operation bodies, while enforcing the final-admission
ordering above. It SHALL preserve existence-before-authorization ordering
(`validation_unknown_target` before `authorization_forbidden`). It composes
with, and does not replace, the origin-side capability model: it is an
independent authority exercised by the receiving trust domain.

The filter SHALL operate at peer-relay granularity. The origin identity carried
as `on_behalf_of` per the `cross-relay-routing` capability's
`Cross-Relay Sender Attribution Forwarding` requirement is advisory: the
receiver authenticates the peer, not the foreign origin. That attribution
SHALL NOT permit or deny a target.

The new scope semantics SHALL NOT change recipient selection, introduce
namespace recipient fanout, bypass transport capabilities, enable onward
forwarding, or add operations such as cross-relay Look. A wildcard grants
coverage only within already-supported ingress operations and SHALL NOT confer
administrative or application-introspection rights.

#### Scenario: In-scope cross-relay target accepted

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@myapp`
- **AND** the peer principal's current scope covers `myapp`
- **THEN** the ingress filter permits the target and delivery proceeds

#### Scenario: Out-of-scope cross-relay target denied

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@secret`
- **AND** its current scope does not cover `secret`
- **THEN** the relay returns `authorization_forbidden` with an ingress-denied
  detail

#### Scenario: Peer with no scope reaches nothing

- **WHEN** a peer with empty or absent scope issues a forwarded target operation
- **THEN** authorization denies every existing target (deny-by-default)

#### Scenario: Attribution does not widen ingress

- **WHEN** a forwarded target operation carries an `on_behalf_of` origin
- **AND** that origin is outside the peer relay principal's scope
- **THEN** the filter evaluates the peer's current scope alone
- **AND** attribution does not permit or deny any target

#### Scenario: One peer reaches multiple explicit namespaces

- **WHEN** one peer connection with scope `alpha,beta` forwards Send/Raww to
  addressable targets in each namespace
- **THEN** both namespaces pass the ingress filter over that connection
- **AND** no per-namespace credential or endpoint is required

#### Scenario: Mixed allowed and denied targets reject before admission

- **WHEN** an ingress Send targets existing principals in alpha and secret
- **AND** the current scope covers only alpha
- **THEN** the entire ingress operation is denied
- **AND** neither target is admitted

#### Scenario: Wildcard includes newly addressable namespace types

- **WHEN** a peer has `scope="*"` and a namespace type becomes addressable
- **THEN** supported operations targeting its principals are in scope immediately
- **AND** no reconnect, reissuance or extra namespace-type grant is required

#### Scenario: Wildcard does not expand operations or onward routing

- **WHEN** a peer has `scope="*"`
- **THEN** existing origin control requirements, target capability checks and
  onward-forwarding restrictions still apply
- **AND** the wildcard does not enable cross-relay Look or administration

#### Scenario: Narrowing wins before admission

- **WHEN** an update removes beta before a prepared ingress operation is
  finally authorized and admitted
- **THEN** the operation cannot use an earlier grant snapshot to reach beta
- **AND** a delivery admitted before that update is not retroactively cancelled

### Requirement: Cross-Relay Discovery Ingress Filtering

The receiving relay SHALL authorize every discovery operation using the current
authoritative scope for the authenticated peer under `Peer Ingress Scope Grammar`
and `Authoritative Peer Scope Updates` in the `relay-identity` capability.
It SHALL reuse peer target namespace coverage, not application introspection
scope matching or a Hello-time peer grant snapshot.

The receiving relay SHALL derive results only from its own existing discovery
candidate sources (currently bundle catalog and GLOBAL registry). It SHALL NOT
use a foreign origin principal, `on_behalf_of`, origin-supplied catalog, or
origin-local relay alias as authorization or discovery input.

Ingress behavior SHALL be:

- explicit namespaces expose their complete addressable principal sets;
- `*` covers every namespace with addressable principals, including GLOBAL,
  future bundles and new addressable namespace types as they are introduced;
- empty or absent scope rejects namespace and principal discovery with
  `authorization_forbidden`;
- a concrete namespace outside scope returns `authorization_forbidden` without
  revealing whether it exists.

Namespace discovery SHALL return sorted unique covered namespaces containing
at least one addressable principal. An empty namespace SHALL be omitted,
producing the same result as an absent namespace. Principal discovery SHALL
continue to name one concrete namespace per request. A covered absent namespace
SHALL return the existing neutral empty bundle view. No wildcard or set scope
SHALL cause foreign principal aggregation or append GLOBAL to a lookup of
another namespace.

Because peer grants cover complete namespaces, covered principal discovery
SHALL return complete listings and their normal namespace diagnostics, with
no scope-induced `principals_partial` marker. The generic marker's contracts
for other uses SHALL remain unchanged. GLOBAL SHALL use its registry-backed
view. Future addressable namespace types SHALL use their own existing candidate
and listing mechanisms; wildcard authorization SHALL require no extra type
allowlist or additional opt-in.

Peer ingress scope SHALL remain operation-agnostic; no separate list control
is introduced for the peer. Origin list authorization and the prohibition on
peer discovery re-forwarding SHALL remain unchanged.

For a nonempty scope covering no discoverable namespace, namespace discovery
SHALL return an empty success and record the scope and requester in local
inscriptions. That record SHALL NOT alter the response or disclose additional
namespace existence to the peer. Credentials SHALL NOT be recorded.

Final filtering SHALL be ordered against scope-update commits. A decision
ordered after update success SHALL use the replacement or a later committed
scope, including on the same connection. A result fixed before commit may be
sent afterwards, without implying authority for a later lookup.

#### Scenario: Namespace scope exposes complete namespace

- **WHEN** peer scope is namespace `myapp`
- **AND** the peer requests namespaces or principals
- **THEN** namespace discovery may include nonempty `myapp`
- **AND** principal discovery for `myapp` returns its complete listing

#### Scenario: Empty namespace under namespace scope is omitted

- **WHEN** peer scope covers `myapp`
- **AND** myapp contains no configured or registered addressable principals
- **THEN** namespace discovery omits myapp
- **AND** does not reveal whether it exists

#### Scenario: Scope covering nothing is recorded rather than refused

- **WHEN** a peer's nonempty scope covers no discoverable namespace
- **THEN** namespace discovery returns empty success, not authorization denial
- **AND** the response does not reveal namespace existence
- **AND** the receiving relay records the scope and requester locally

#### Scenario: Absent scope denies discovery

- **WHEN** an authenticated peer has no registered scope
- **THEN** namespace and principal discovery return `authorization_forbidden`

#### Scenario: Out-of-scope namespace reveals no existence

- **WHEN** a peer requests principals for a namespace outside its current scope
- **THEN** the relay returns `authorization_forbidden`
- **AND** the response does not reveal whether that namespace exists

#### Scenario: Set filters namespace discovery without aggregating principals

- **WHEN** scope is `alpha,beta` and alpha, beta and secret contain principals
- **THEN** namespace discovery returns alpha and beta only
- **AND** separate concrete-namespace requests return complete alpha or beta
  views, including normal diagnostics and no partial marker

#### Scenario: Wildcard discovers GLOBAL and future addressable candidates

- **WHEN** scope is `*` and GLOBAL, a new bundle and a newly addressable type
  each provide a principal through the existing discovery candidate mechanism
- **THEN** namespace discovery includes all three without a grant change

#### Scenario: Update applies to both discovery operations on the same connection

- **WHEN** scope changes from `*` to `alpha` without disconnecting the peer
- **THEN** subsequent namespace discovery returns only covered candidates
- **AND** subsequent principal lookup of GLOBAL or beta is denied
- **AND** a result fixed before the commit can still arrive afterwards

#### Scenario: Clearing scope removes discovery rights

- **WHEN** an administrator commits empty scope for a connected peer
- **THEN** subsequent namespace and principal discovery decisions are denied
- **AND** the peer retains its credential and connection
