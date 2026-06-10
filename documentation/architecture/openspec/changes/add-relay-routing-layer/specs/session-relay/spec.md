## MODIFIED Requirements

### Requirement: Suffix-Based Target Routing

The relay SHALL require every target on a target-addressed operation (Send, Look,
Raww) to be a **fully-qualified** principal ID carrying an `@<namespace>` suffix,
and SHALL derive the routing registry from that suffix alone:

- Target with `@GLOBAL` suffix → relay-wide registry (`RegistryKey::RelayWide`)
- Target with `@<bundle>` suffix → bundle registry for `<bundle>`
- Target with no suffix (bare) → rejected with `validation_unqualified_target`

The relay SHALL NOT resolve a bare target against the sender's bound bundle or
the UI registry, regardless of sender type. The bare-id convenience is a
client-side concern: a client (or its MCP server) fills in the namespace before
sending, so the relay always receives fully-qualified targets and classifies
them from the suffix without consulting bundle configuration. The relay SHALL
NOT require a separate `namespace` field to route to qualified targets; the
suffix is authoritative.

Routing is **per-target by namespace**: each target is delivered in the
namespace its suffix names — a bundle via the catalog, `GLOBAL` via the session
registry. The relay SHALL NOT assign a sender a single "routing bundle" derived
from its targets. In particular a relay-wide (`GLOBAL`) sender SHALL NOT be
required to supply a bundle-qualified target: a `Send` from a relay-wide sender
to `@GLOBAL`-only targets routes through the registry and SHALL NOT return
`validation_missing_routing_namespace`. A single `Send` request MAY mix
relay-wide (`@GLOBAL`) and bundle-session targets; the relay SHALL fan out
delivery to each target in its own namespace.

Any authenticated session (bundle-bound or relay-wide) MAY send to `@GLOBAL`
targets. This is a routing invariant, not a relaxation of the scope ladder: a
relay-wide (`@GLOBAL`) target is delivered through the session registry (keyed
by `principal_id`) rather than by crossing into a peer bundle, so it classifies
at the `all:home` tier under the Uniform Cross-Bundle Authorization Model and
never demands `all:all`. This holds whether the sender is bundle-bound (an
agent replying to the operator) or itself relay-wide (one relay-wide principal
messaging another). It is asymmetric with a relay-wide *requester* reaching
*into* a bundle, which the uniform model classifies as cross-namespace and which
therefore does require `all:all`.

#### Scenario: Bundle-bound agent sends to @GLOBAL operator

- **WHEN** a session principal sends `Send` with
  `targets = ["operator@GLOBAL"]`
- **AND** `operator@GLOBAL` is registered as a relay-wide session
- **THEN** relay delivers the message to `operator@GLOBAL`

#### Scenario: @GLOBAL principal sends to bundle session

- **WHEN** a relay-wide principal sends `Send` with
  `targets = ["agent@bundle-a"]`
- **THEN** relay routes to bundle `bundle-a` and delivers to `agent`

#### Scenario: @GLOBAL principal rawws to bundle session under all:all

- **WHEN** a relay-wide principal issues a Raww request with
  `target_session = "agent@bundle-a"`
- **AND** the requester's configured `raww` scope is `all:all`
- **THEN** relay routes to bundle `bundle-a` and delivers to `agent`

#### Scenario: Bare target rejected as unqualified

- **WHEN** any sender (bundle-bound or relay-wide) issues a target-addressed
  request with a bare target (no `@<namespace>` suffix)
- **THEN** relay returns `validation_unqualified_target`
- **AND** relay does not resolve the target against the sender's bound bundle or
  the UI registry

#### Scenario: Mixed relay-wide and bundle targets fan out

- **WHEN** a sender includes both an `@GLOBAL` target and a `@<bundle>` target
  in the same `Send` request
- **THEN** relay delivers to `@GLOBAL` via the relay-wide registry and to the
  `@<bundle>` target via the bundle catalog

#### Scenario: Relay-wide sender to GLOBAL-only targets needs no bundle

- **WHEN** a relay-wide (`GLOBAL`) sender issues a `Send` whose targets are all
  `@GLOBAL`
- **THEN** relay routes each target through the relay-wide registry
- **AND** relay does not require a bundle-qualified target and does not return
  `validation_missing_routing_namespace`

### Requirement: Relay raww target resolution and bundle boundary

Relay raww target resolution SHALL use canonical session id identifiers only.
Cross-bundle reach is governed by the requester's configured `raww` scope and
the Uniform Cross-Bundle Authorization Model; there is no hard routing rejection
for raww targeting a different bundle.

Validation behavior:
- bare/unqualified target (no `@<namespace>` suffix) → `validation_unqualified_target`
- relay-wide (`@GLOBAL`) or reserved (`@EXTERNAL`/`@RELAY`) target →
  `validation_unsupported_namespace` (such a target names no session that accepts
  raw input; uniform with the Look single-target stage)
- unknown/non-canonical target → `validation_unknown_target`
- cross-bundle raww with insufficient scope → `authorization_forbidden`

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage (`resolve_target`); their relay-wide/reserved
target rejection is uniform. A richer, transport-class-specific rejection is
intentionally deferred to session-attribute-based routing.

#### Scenario: Reject unknown raww target

- **WHEN** caller invokes `raww` with a target token that is not a canonical
  configured session id
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

#### Scenario: Reject relay-wide raww target as unsupported namespace

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target
- **THEN** relay returns `validation_unsupported_namespace`
- **AND** the rejection is uniform with the Look stage for the same target

#### Scenario: Cross-bundle raww denied by scope

- **WHEN** caller invokes `raww` with a target in a different bundle
- **AND** requester's `raww` scope is `all:home` or narrower
- **THEN** relay returns `authorization_forbidden`

### Requirement: Relay raww authorization mapping

Relay SHALL evaluate raww authorization using policy control `raww`.

Policy scope contract:
- allowed values: `none`, `self`, `all:home`, `all:all`
- invalid values (unknown values) SHALL fail configuration validation with
  `validation_invalid_policy_scope`

When raww is denied by policy, relay SHALL return `authorization_forbidden`
with canonical minimum details:
- `capability` = `raww.write`
- `requester_session`
- `bundle_name`
- `reason`

#### Scenario: Deny raww under self scope for non-self target

- **WHEN** requester policy sets `raww = "self"`
- **AND** requester invokes raww to another session in the same bundle
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

#### Scenario: Cross-bundle raww permitted under all:all

- **WHEN** requester policy sets `raww = "all:all"`
- **AND** requester invokes raww to a session in a different bundle
- **THEN** relay routes to the target and delivers

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's identity and policy controls in the
requester's **home namespace** — its bound bundle's policy for a session
principal, or the operator policy for a relay-wide principal — classify the
requester-to-target relationship relative to that home namespace, and require a
scope tier on the policy scope ladder:

- self target → `self`
- same-namespace non-self target → `all:home`
- other-namespace target → `all:all`

A principal's home namespace SHALL be its native namespace: a session's home is
its bundle, and a relay-wide principal's home is its reserved namespace
(`GLOBAL` / `EXTERNAL` / `RELAY`). `all:home` SHALL therefore confer authority
only within the principal's own namespace; a relay-wide principal (for example a
`@GLOBAL` operator) SHALL require `all:all` to reach into any bundle, since a
bundle is not its home namespace. There SHALL be no global/relay-principal
exemption from this threshold.

This requester-axis rule has a target-axis counterpart: a relay-wide
(`@GLOBAL`) *target* SHALL classify at the `all:home` tier rather than `all:all`,
because relay-wide principals are delivered through the session registry rather
than by crossing into a peer bundle (see Suffix-Based Target Routing). Reaching a
relay-wide target — an agent messaging the operator, or one relay-wide principal
messaging another — is therefore not a cross-namespace act and SHALL NOT demand
`all:all`. This is a routing invariant, not a per-operation policy exemption.

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-namespace policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer namespace
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer namespace SHALL NOT be required, and the
relay SHALL NOT resolve or authorize the requester in a target's (or any other
borrowed) bundle in place of its home namespace, on any target operation.

Whether a capability can be configured to a cross-bundle (`all:all`) scope SHALL
be governed by the policy schema's per-capability allowed-scope set, not by
relay routing code. A capability whose schema cap is below `all:all` SHALL
therefore be unreachable cross-bundle until the policy schema is widened, with
no code override involved.

#### Scenario: Requester authorized in dispatch bundle, not peer bundle

- **WHEN** a session in bundle A issues a cross-bundle operation targeting
  bundle B
- **THEN** relay evaluates the requester's policy controls from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-namespace session raww/look authorizes in the home namespace

- **WHEN** a session in bundle A issues a `raww` or `look` targeting a session
  in bundle B
- **AND** the requester's configured scope for that capability in bundle A's
  policy is `all:all`
- **THEN** relay resolves and authorizes the requester in bundle A (its home
  namespace) and the operation succeeds against the bundle B target
- **AND** relay does not resolve the requester in bundle B and does not return
  `validation_unknown_sender` for a requester unknown there

#### Scenario: Cross-bundle operation denied under home scope

- **WHEN** a requester issues a cross-bundle `look`, `send`, or `list`
- **AND** the requester's configured scope for that capability is `all:home` or
  narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Cross-bundle list enumerates peer bundle under all-all scope

- **WHEN** a requester with `list = all:all` lists a configured peer bundle's
  sessions
- **THEN** relay returns the peer bundle's session listing rather than rejecting
  the requester as unknown

#### Scenario: Relay-wide principal needs all-all to reach a bundle

- **WHEN** a relay-wide principal (for example a `@GLOBAL` operator) issues a
  `list` or `send` targeting a bundle namespace
- **AND** its configured scope for that capability is `all:home`
- **THEN** relay returns `authorization_forbidden`, because the bundle is not the
  principal's home (`GLOBAL`) namespace
- **AND** the same principal under `all:all` is permitted

#### Scenario: Capability not configurable to cross-bundle scope fails uniformly

- **WHEN** a requester issues a cross-bundle request for a capability whose
  policy-schema cap is below `all:all`
- **THEN** the request fails the uniform `all:all` threshold with
  `authorization_forbidden`
- **AND** no operation-specific code override is involved
