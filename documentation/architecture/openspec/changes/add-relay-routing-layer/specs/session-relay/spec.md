## MODIFIED Requirements

### Requirement: Suffix-Based Target Routing

The relay SHALL infer the routing bundle for all target-addressed operations
(Send, Look, Raww) from the `@<namespace>` suffix of the target's principal ID:

- Target with `@GLOBAL` suffix → relay-wide registry (`RegistryKey::RelayWide`)
- Target with `@<bundle>` suffix → bundle registry for `<bundle>`
- Bare target (no suffix) with bundle-bound sender → sender's bound bundle
- Bare target (no suffix) with relay-wide sender → error if no namespace
  supplied

The relay SHALL NOT require an explicit `namespace` field from the client to
route to relay-wide (`@GLOBAL`) targets or to bundle-qualified targets. Clients
specify targets as fully-qualified principal IDs; the relay derives the routing
registry from the suffix.

A single `Send` request MAY mix relay-wide (`@GLOBAL`) and bundle-session
targets; the relay SHALL fan out delivery to each target in its own namespace.

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

#### Scenario: Bare target defaults to sender's bound bundle

- **WHEN** a bundle-bound session sends a target-addressed request with a bare
  target (no `@<namespace>` suffix)
- **THEN** relay resolves the target within the sender's bound bundle

#### Scenario: Relay-wide sender with bare target returns error

- **WHEN** a relay-wide principal issues a target-addressed request with a bare
  target (no suffix)
- **THEN** relay returns `validation_missing_routing_namespace`

#### Scenario: Mixed relay-wide and bundle targets fan out

- **WHEN** a sender includes both an `@GLOBAL` target and a `@<bundle>` target
  in the same `Send` request
- **THEN** relay delivers to `@GLOBAL` via the relay-wide registry and to the
  `@<bundle>` target via the bundle catalog

### Requirement: Relay raww target resolution and bundle boundary

Relay raww target resolution SHALL use canonical session id identifiers only.
Cross-bundle reach is governed by the requester's configured `raww` scope and
the Uniform Cross-Bundle Authorization Model; there is no hard routing rejection
for raww targeting a different bundle.

Validation behavior:
- unknown/non-canonical target → `validation_unknown_target`
- cross-bundle raww with insufficient scope → `authorization_forbidden`

Validation precedence SHALL evaluate target existence before authorization
policy checks.

#### Scenario: Reject unknown raww target

- **WHEN** caller invokes `raww` with a target token that is not a canonical
  configured session id
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

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
