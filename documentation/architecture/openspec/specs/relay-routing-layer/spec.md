# relay-routing-layer Specification

## Purpose
The shared resolution and authorization stages that every target-addressed operation (Send, Look, Raww, List) flows through before reaching its operation-specific body. The spec governs suffix-based target classification (every target MUST carry an `@<namespace>` suffix; bare ids are rejected at the resolution stage without consulting bundle configuration) and uniform cross-bundle authorization evaluated in the requester's home namespace (no per-operation cross-bundle logic; the policy schema's per-capability allowed-scope set is the sole authority for cross-bundle reach). Operation bodies SHALL exclude routing and authorization logic — they receive a fully-resolved and authorized `ResolvedRoute`.
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

Whether a capability can ever be configured to reach the cross-namespace (`all`)
tier is governed solely by the policy schema's per-capability allowed-scope set
(`parse_policy_controls`). The relay SHALL NOT apply per-operation
cross-namespace logic in handler or routing code; this data-driven spine —
uniform tier classification plus the schema allowed-scope set — SHALL be the
single authority for cross-namespace reach.

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

#### Scenario: Cross-bundle Raww denied under home

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`

Note: for a relay-wide (`@GLOBAL`) principal, `home` covers only the `GLOBAL`
namespace, which is populated by relay-wide sessions. Sessions whose registry
entry carries `can_be_written = false` are rejected by the raww capability gate,
so `home` confers no effective raww reach to those targets. `all` remains the
meaningful tier for cross-bundle raww from a relay-wide principal.

#### Scenario: Cross-bundle Raww permitted under all

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `all`
- **THEN** relay routes to the target's bundle and delivers

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
notation `<session_id>@<bundle_name>!<relay_id>` for the delivery operations
`Send` and `Raww`. The `!<relay_id>` suffix SHALL be parsed before the
`@<namespace>` split; `<relay_id>` is the local `alias` of a configured
`[[peers]]` entry (this relay's own name for the peer, no `@RELAY` suffix). A
target carrying a `!<relay_id>` suffix SHALL be classified as a **cross-relay
target** carrying the peer `relay_id` and the foreign `session_id@bundle_name`.

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

#### Scenario: Cross-relay target requires origin all-tier authorization

- **WHEN** a session issues a `Send` or `Raww` to a `!<relay_id>` target
- **AND** the requester's configured scope for the operation is `home` or narrower
- **THEN** the relay returns `authorization_forbidden`

#### Scenario: Malformed bang-path rejected at resolution

- **WHEN** a target carries a `!<relay_id>` suffix with an empty `<relay_id>` or
  a missing `@<bundle_name>` segment
- **THEN** the resolution stage rejects it with a structured validation error
  without consulting configuration

### Requirement: Cross-Relay Target Ingress Filter

The authorization stage SHALL apply a target-side ingress filter, in addition to
the uniform scope-tier check, whenever a target-addressed operation's requester
is a relay principal (`<id>@RELAY`) — i.e. an inbound request forwarded by a peer
relay. The ingress filter SHALL authorize each resolved target against the peer
relay principal's registered `scope` — the value recorded on the principal store
record when the peer credential is registered via `new peer <id>@RELAY`,
evaluated with the existing scope-permits check:

- A target whose namespace or canonical principal id is covered by the peer's
  `scope` SHALL be permitted.
- The posture SHALL be **deny-by-default**: a peer principal with an empty or
  absent `scope` covers no target, and any target outside the scope SHALL be
  rejected with `authorization_forbidden` carrying an ingress-denied detail.

The ingress filter SHALL be evaluated at the shared `authorize_route` stage — the
single seam every target operation passes through — not within individual
operation bodies, and SHALL preserve the existence-before-authorization ordering
(`validation_unknown_target` before `authorization_forbidden`). The ingress
filter composes with, and does not replace, the origin-side capability model: it
is an independent authority exercised by the receiving trust domain.

This slice filters at peer-relay granularity (the `<id>@RELAY` principal's
scope). Distinguishing which originating principal *inside* the peer relay is
acting is out of scope; carrying the original sender identity across the boundary
(the reserved `on_behalf_of` field) is deferred to a follow-on, so this slice
gates solely on the peer relay principal.

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

