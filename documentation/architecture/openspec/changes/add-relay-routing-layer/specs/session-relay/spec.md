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
