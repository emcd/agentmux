# relay-routing-layer Specification

## Purpose
The shared resolution and authorization stages that every target-addressed operation (Send, Look, Raww, List) flows through before reaching its operation-specific body. The spec governs suffix-based target classification (every target MUST carry an `@<namespace>` suffix; bare ids are rejected at the resolution stage without consulting bundle configuration) and uniform cross-bundle authorization evaluated in the requester's home namespace (no per-operation cross-bundle logic; the requester's configured scope, checked against the uniform tier, is the sole authority for cross-bundle reach). Operation bodies SHALL exclude routing and authorization logic — they receive a fully-resolved and authorized `ResolvedRoute`.
## Requirements
### Requirement: Routing Resolution Stage

The relay SHALL resolve all target-addressed operations (Send, Look, Raww, List)
through a shared, operation-agnostic resolution stage before invoking any
operation handler. The resolution stage SHALL operate **without consulting bundle
configuration or the bundle catalog** — it classifies targets from their
principal-ID suffixes alone — and SHALL:

- Parse each target's `@<namespace>` suffix into a `ResolvedTarget` containing
  the target's canonical `principal_id`, namespace, and bare session id. An
  `@GLOBAL` suffix names namespace `GLOBAL`; an `@<bundle>` suffix names that
  bundle namespace; the bare session id is the portion before the suffix.
- Reject a target that carries no suffix with `validation_unqualified_target`.
  The stage never resolves a bare id against bundle membership or the UI
  registry.
- Identify the requester's dispatch (home) namespace: the sender's bound bundle
  for session principals, or `GLOBAL` for relay-wide principals.
- Return a `ResolvedRoute { dispatch_namespace, requester_session, targets }` to
  the authorization stage.

Target existence, transport capabilities, readiness, and runtime/delivery
binding are NOT resolved here; they are registry/configuration concerns handled
by the operation body after route classification and before authorization (see
Operation Body Contract).

#### Scenario: Single target classified from its suffix

- **WHEN** the relay receives a Look or Raww request targeting `agent@bundle-a`
- **THEN** the resolution stage produces a target with
  `principal_id = agent@bundle-a`, `namespace = bundle-a`, and
  `session_id = agent` without loading bundle configuration
- **AND** `dispatch_namespace` is the requester's home namespace

#### Scenario: Multi-target Send classified per target

- **WHEN** the relay receives a Send request with targets in multiple namespaces
- **THEN** the resolution stage produces one `ResolvedTarget` per target, each
  classified to its own namespace from its suffix

#### Scenario: Unqualified target rejected at resolution

- **WHEN** a target-addressed request carries a bare target (no `@<namespace>`
  suffix)
- **THEN** the resolution stage returns `validation_unqualified_target` without
  loading any bundle configuration

### Requirement: Authorization Stage

The relay SHALL evaluate authorization for all target-addressed operations
through a shared authorization stage that receives the `ResolvedRoute` from the
resolution stage. The authorization stage SHALL:

- Resolve the requester's policy controls from its **home namespace's** policy:
  a bundle's session policy for a session principal, or the `GLOBAL` operator
  policy for a relay-wide principal. The home namespace is derived from the
  requester's own principal id, never from a target or a borrowed peer bundle. A
  relay-wide (`GLOBAL`) sender is authorized from the operator policy and is
  never assigned a bundle namespace.
- Classify each target's relationship to the requester into a uniform scope tier
  and require the maximum tier across the route: `self` for a self-target,
  `home` for a same-namespace target, `all` for a target in a peer namespace. A
  relay-wide (`@GLOBAL`) target is delivered through the unified registry in its
  own namespace, not by crossing into a peer bundle, so it classifies at the
  `home` tier rather than raising the requirement to `all`.
- Consider **every** target when computing the required tier; authorization is
  the maximum across the whole route, never a single representative target.
  Because the scope ladder is monotone, a requester whose configured scope
  satisfies the maximum tier is thereby authorized for every target. The check is
  **all-or-nothing**: if the requester's scope does not satisfy the maximum, the
  entire operation is rejected with `authorization_forbidden` and no target is
  delivered. (Per-target partial delivery is a deferred follow-up.)
- Check the required tier against the requester's configured scope for the
  operation's capability. Each operation contributes only an `OperationProfile`
  (its capability and addressing mode); it carries no per-operation
  cross-namespace policy.

The relay SHALL NOT apply per-operation cross-namespace logic in handler or
routing code. This data-driven spine — uniform tier classification checked
against the requester's configured scope — SHALL be the single authority for
cross-namespace reach.

#### Scenario: Requester authorized in home namespace for cross-bundle Raww

- **WHEN** a session in bundle A issues a Raww request targeting bundle B
- **THEN** relay evaluates the requester's `raww` policy control from bundle A
  (its home namespace)
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Relay-wide sender authorized from the operator policy

- **WHEN** a relay-wide (`GLOBAL`) sender issues a target-addressed operation
- **THEN** relay resolves the requester's controls from the `GLOBAL` operator
  policy, not from any bundle
- **AND** does not borrow a peer bundle's namespace for the sender

#### Scenario: Relay-wide send to GLOBAL-only targets requires no bundle

- **WHEN** a relay-wide (`GLOBAL`) sender issues a Send whose targets are all
  `@GLOBAL`
- **THEN** relay routes each target through the unified registry
- **AND** does not return `validation_missing_routing_namespace`

#### Scenario: Cross-bundle List resolves requester in home bundle

- **WHEN** a session in bundle A issues a List request enumerating bundle B
- **THEN** relay evaluates the requester's `list` policy control from bundle A
- **AND** does not return `validation_unknown_sender` because the requester is not
  a member of bundle B

### Requirement: Operation Body Contract

Operation handler bodies SHALL receive a `ResolvedRoute` whose targets are
already classified by namespace and authorized. Handler bodies SHALL NOT:

- Parse `@<namespace>` suffixes from principal IDs.
- Evaluate requester policy controls or classify target scope tiers.

Handler bodies MAY look up target entries in the unified session registry and MAY
load target bundle configuration to validate configured membership. Delivery and
runtime binding SHALL come from the registry entry once it exists. This is
existence, capability, readiness, and delivery work, distinct from routing and
authorization. They SHALL implement only operation-specific work: existence
validation, capability checks, readiness handling, snapshot capture, delivery
enqueueing, raw text injection, session enumeration, or lifecycle control.

#### Scenario: Handler body free of routing and authorization logic

- **WHEN** a developer reads any target-operation handler (`handle_send`,
  `handle_look`, `handle_raww`, `handle_list`)
- **THEN** no principal-ID suffix parsing and no requester-policy or scope-tier
  evaluation are present
- **AND** routing classification and authorization are handled exclusively by the
  dispatch layer, with only existence validation, capability checks, readiness
  handling, and delivery assembly remaining in the body

### Requirement: Cross-Relay Target Classification

The routing resolution stage SHALL recognize the cross-relay bang-path target
notation `<principal_id>!<relay_id>` for the delivery operations `Send` and
`Raww`. The `!<relay_id>` suffix SHALL be parsed before the `@<namespace>` split;
`<relay_id>` is the local `alias` of a configured `[[peers]]` entry (this relay's
own name for the peer, no `@RELAY` suffix). A target carrying a `!<relay_id>`
suffix SHALL be classified as a **cross-relay target** carrying the peer
`relay_id` and the foreign `<principal_id>`.

The foreign principal SHALL be accepted when it is a bundle-qualified session or
a relay-wide `@GLOBAL` principal, and not only the former. Cross-relay forwarding
attributes any verified requester, and a relay-wide user is one, so restricting
resolution to bundle sessions would make a correctly delivered sender unparseable
as a target and break the reply path the envelope form exists to provide.

The namespaces that name no routable recipient locally — the application and peer
relay partitions — SHALL continue to be rejected as unsupported, and an
unqualified principal SHALL continue to be rejected as such. Widening reaches the
principal kinds a conforming forwarding relay can attribute, not every string a
peer might assert; a delivered sender may carry an origin outside that set, and a
reply to it is expected to fail here rather than to resolve. A target SHALL also
be rejected when the `relay_id` is empty or itself contains a separator.

Classification SHALL remain configuration-free: the resolution stage SHALL NOT
consult `[[peers]]` or any catalog to classify a cross-relay target. The
existence of the named peer is a delivery-time concern validated by the operation
body, not the resolver — mirroring how an unknown local bundle surfaces at
delivery rather than resolution.

A cross-relay target is cross-namespace with respect to the requester's home
namespace by construction, so it SHALL classify at the `all` scope tier. The
origin-side authorization stage is unchanged: the requester's configured
`send` / `raww` scope MUST reach `all` for the operation to be authorized on the
originating relay.

#### Scenario: Cross-relay Send target classified from bang-path

- **WHEN** the relay receives a `Send` targeting `claude@myapp!peer-relay`
- **THEN** the resolution stage classifies a cross-relay target with
  `relay_id = peer-relay` and foreign principal `claude@myapp`
- **AND** does so without consulting `[[peers]]` or the bundle catalog

#### Scenario: Cross-relay target carrying a relay-wide origin

- **WHEN** the relay receives a `Send` targeting `operator@GLOBAL!peer-relay`
- **THEN** the resolution stage classifies a cross-relay target with
  `relay_id = peer-relay` and foreign principal `operator@GLOBAL`

#### Scenario: Cross-relay target requires origin all-tier authorization

- **WHEN** a session issues a `Send` or `Raww` to a `!<relay_id>` target
- **AND** the requester's configured scope for the operation is `home` or narrower
- **THEN** the relay returns `authorization_forbidden`

#### Scenario: Cross-relay target in a non-routable namespace still rejected

- **WHEN** the relay receives a `Send` targeting a principal qualified with the
  application or peer relay namespace and a `!<relay_id>` suffix
- **THEN** the resolution stage rejects it as an unsupported namespace

#### Scenario: Malformed bang-path rejected at resolution

- **WHEN** a target carries a `!<relay_id>` suffix with an empty `<relay_id>` or
  an unqualified principal
- **THEN** the resolution stage rejects it with a structured validation error
  without consulting configuration

### Requirement: Cross-Relay Target Ingress Filter

The authorization stage SHALL apply a target-side ingress filter to delivery
from an authenticated peer relay (`<id>@RELAY`). The receiving relay SHALL use
that peer's current registered scope rather than a receiving-bundle policy for a
foreign sender. Origin-side uniform scope-tier authorization SHALL remain
independently required on the forwarding relay.

The filter SHALL authorize every resolved target against the current peer record
using `Peer Ingress Scope Grammar` and `Authoritative Peer Scope Updates` in the
`relay-identity` capability:

- A comma-separated namespace set SHALL cover all addressable targets in those
  namespaces only.
- `*` SHALL cover every namespace with addressable principals, including GLOBAL,
  future bundles and newly introduced addressable namespace types immediately.
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
as `on_behalf_of` per the `cross-relay-routing` capability's Cross-Relay Sender
Attribution Forwarding requirement is advisory: the receiver authenticates the
peer, not the foreign origin. That attribution SHALL NOT permit or deny a target.

The new scope semantics SHALL NOT change recipient selection, introduce
namespace recipient fanout, bypass transport capabilities, enable onward
forwarding, or add operations such as cross-relay Look. A wildcard grants
coverage only within already-supported ingress operations and SHALL NOT confer
administrative or application-introspection rights.

#### Scenario: In-scope cross-relay target accepted

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@myapp`
- **AND** the peer principal's registered `scope` covers `myapp`
- **THEN** the ingress filter permits the target and delivery proceeds

#### Scenario: Out-of-scope cross-relay target denied

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@secret`
- **AND** the peer principal's registered `scope` does not cover `secret`
- **THEN** the relay returns `authorization_forbidden` with an ingress-denied
  detail

#### Scenario: Peer with no scope reaches nothing

- **WHEN** a peer relay principal with an empty or absent `scope` issues a
  forwarded target operation
- **THEN** the relay returns `authorization_forbidden` for every target
  (deny-by-default)

#### Scenario: Attribution does not widen ingress

- **WHEN** a forwarded target operation carries an `on_behalf_of` naming an
  origin principal
- **AND** that origin is outside the peer relay principal's registered `scope`
- **THEN** the ingress filter still evaluates the peer relay principal's scope
  alone
- **AND** the attribution does not permit or deny any target

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
- **THEN** namespace discovery may include `myapp`
- **AND** principal discovery for `myapp` returns its complete listing

#### Scenario: Empty namespace under namespace scope is omitted

- **WHEN** peer scope covers `myapp`
- **AND** myapp contains no configured or registered addressable principals
- **THEN** namespace discovery omits `myapp`
- **AND** does not reveal whether `myapp` exists

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
- **AND** the response does not reveal whether the namespace exists

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
