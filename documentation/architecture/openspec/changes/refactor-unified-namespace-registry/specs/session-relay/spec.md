## MODIFIED Requirements

### Requirement: Suffix-Based Target Routing

The relay SHALL require every target on a target-addressed operation (Send, Look,
Raww) to be a **fully-qualified** principal ID carrying an `@<namespace>` suffix,
and SHALL derive the routing namespace from that suffix alone:

- Target with `@GLOBAL` suffix -> namespace `GLOBAL`
- Target with `@<bundle>` suffix -> namespace `<bundle>`
- Target with no suffix (bare) -> rejected with `validation_unqualified_target`

The relay SHALL NOT resolve a bare target against the sender's bound bundle or
the UI registry, regardless of sender type. The bare-id convenience is a
client-side concern: a client (or its MCP server) fills in the namespace before
sending, so the relay always receives fully-qualified targets and classifies
them from the suffix without consulting bundle configuration. The relay SHALL
NOT require a separate `namespace` field to route to qualified targets; the
suffix is authoritative.

Routing is **per-target by namespace**: each target is delivered in the
namespace its suffix names. The relay SHALL use one session registry keyed by the
target's canonical `principal_id` for target lookup rather than selecting between
a bundle registry and a relay-wide registry. The relay SHALL NOT assign a sender
a single "routing bundle" derived from its targets. In particular a relay-wide
(`GLOBAL`) sender SHALL NOT be required to supply a bundle-qualified target: a
`Send` from a relay-wide sender to `@GLOBAL`-only targets routes through the
unified registry and SHALL NOT return `validation_missing_routing_namespace`. A
single `Send` request MAY mix relay-wide (`@GLOBAL`) and bundle-session targets;
the relay SHALL fan out delivery to each target in its own namespace.

Any authenticated session (bundle-bound or relay-wide) MAY send to `@GLOBAL`
targets. This is a routing invariant, not a relaxation of the scope ladder: a
relay-wide (`@GLOBAL`) target is delivered through the unified registry in its
own namespace rather than by crossing into a peer bundle, so it classifies at the
`home` tier under the Uniform Cross-Bundle Authorization Model and never demands
`all`. This holds whether the sender is bundle-bound (an agent replying to the
operator) or itself relay-wide (one relay-wide principal messaging another). It
is asymmetric with a relay-wide *requester* reaching *into* a bundle, which the
uniform model classifies as cross-namespace and which therefore does require
`all`.

#### Scenario: Bundle-bound agent sends to @GLOBAL operator

- **WHEN** a session principal sends `Send` with
  `targets = ["operator@GLOBAL"]`
- **AND** `operator@GLOBAL` is registered in the unified session registry
- **THEN** relay delivers the message to `operator@GLOBAL`

#### Scenario: @GLOBAL principal sends to bundle session

- **WHEN** a relay-wide principal sends `Send` with
  `targets = ["agent@bundle-a"]`
- **THEN** relay routes to namespace `bundle-a` and delivers to `agent@bundle-a`

#### Scenario: @GLOBAL principal rawws to bundle session under all

- **WHEN** a relay-wide principal issues a Raww request with
  `target_session = "agent@bundle-a"`
- **AND** the requester's configured `raww` scope is `all`
- **THEN** relay routes to namespace `bundle-a` and delivers to `agent@bundle-a`

#### Scenario: Bare target rejected as unqualified

- **WHEN** any sender (bundle-bound or relay-wide) issues a target-addressed
  request with a bare target (no `@<namespace>` suffix)
- **THEN** relay returns `validation_unqualified_target`
- **AND** relay does not resolve the target against the sender's bound bundle or
  the UI registry

#### Scenario: Mixed relay-wide and bundle targets fan out

- **WHEN** a sender includes both an `@GLOBAL` target and an `@<bundle>` target
  in the same `Send` request
- **THEN** relay looks up each target by canonical `principal_id` in the unified
  registry
- **AND** both registry entries are the source of truth for delivery bindings,
  with configured membership used only for existence validation
- **AND** delivery preserves each target's own namespace

#### Scenario: Relay-wide sender to GLOBAL-only targets needs no bundle

- **WHEN** a relay-wide (`GLOBAL`) sender issues a `Send` whose targets are all
  `@GLOBAL`
- **THEN** relay routes each target through the unified registry
- **AND** relay does not require a bundle-qualified target and does not return
  `validation_missing_routing_namespace`

### Requirement: GLOBAL Namespace List

The relay SHALL enumerate every principal registered in namespace `GLOBAL` when
`List` is requested with `namespace = "GLOBAL"`, just like any bundle namespace —
including declared-but-offline principals, not only connected ones.

The result SHALL be derived by filtering the unified session registry for entries
whose namespace is `GLOBAL`. The relay SHALL NOT use a separate relay-wide
registry key or a dedicated relay-wide listing path. Each listed principal's
readiness SHALL be computed at list-generation time from the entry's connection
state (a relay-wide principal is ready iff online); a declared-but-offline
principal SHALL be listed with `ready = false` rather than omitted.

#### Scenario: List relay-wide sessions

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** one or more sessions in namespace `GLOBAL` are currently registered
- **THEN** relay returns `RelayResponse::List` containing those sessions

#### Scenario: List includes declared-but-offline relay-wide principal

- **WHEN** relay-wide principal `operator@GLOBAL` is declared in `users.toml` but
  has not connected
- **AND** a principal sends `List` with `namespace = "GLOBAL"`
- **THEN** relay returns `RelayResponse::List` containing `operator@GLOBAL` with
  `ready = false` (offline is a state, not absence), not an empty set

#### Scenario: List with no relay-wide sessions registered

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** no principals are registered in namespace `GLOBAL`
- **THEN** relay returns `RelayResponse::List` with an empty session set

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's identity and policy controls in the
requester's **home namespace** — its bound bundle's policy for a session
principal, or the operator policy for a relay-wide principal — classify the
requester-to-target relationship relative to that home namespace, and require a
scope tier on the policy scope ladder:

- self target -> `self`
- same-namespace non-self target -> `home`
- other-namespace target -> `all`

A principal's home namespace SHALL be its native namespace: a session's home is
its bundle, and a relay-wide principal's home is its reserved namespace
(`GLOBAL` / `EXTERNAL` / `RELAY`). `home` SHALL therefore confer authority only
within the principal's own namespace; a relay-wide principal (for example a
`@GLOBAL` operator) SHALL require `all` to reach into any bundle, since a bundle
is not its home namespace. There SHALL be no global/relay-principal exemption
from this threshold.

This requester-axis rule has a target-axis counterpart: a relay-wide (`@GLOBAL`)
*target* SHALL classify at the `home` tier rather than `all`, because it is
delivered through the unified registry in its own namespace rather than by
crossing into a peer bundle. Reaching a relay-wide target — an agent messaging
the operator, or one relay-wide principal messaging another — is therefore not a
cross-namespace act and SHALL NOT demand `all`. This is a routing invariant, not
a per-operation policy exemption.

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-namespace policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer namespace
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer namespace SHALL NOT be required, and the
relay SHALL NOT resolve or authorize the requester in a target's (or any other
borrowed) bundle in place of its home namespace, on any target operation.

Whether a capability can be configured to a cross-bundle (`all`) scope SHALL be
governed by the policy schema's per-capability allowed-scope set, not by relay
routing code. A capability whose schema cap is below `all` SHALL therefore be
unreachable cross-bundle until the policy schema is widened, with no code
override involved.

#### Scenario: Requester authorized in dispatch bundle, not peer bundle

- **WHEN** a session in bundle A issues a cross-bundle operation targeting
  bundle B
- **THEN** relay evaluates the requester's policy controls from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-namespace session raww/look authorizes in the home namespace

- **WHEN** a session in bundle A issues a `raww` or `look` targeting a session in
  bundle B
- **AND** the requester's configured scope for that capability in bundle A's
  policy is `all`
- **THEN** relay resolves and authorizes the requester in bundle A (its home
  namespace) and the operation succeeds against the bundle B target
- **AND** relay does not resolve the requester in bundle B and does not return
  `validation_unknown_sender` for a requester unknown there

#### Scenario: Cross-bundle operation denied under home scope

- **WHEN** a requester issues a cross-bundle `look`, `send`, or `list`
- **AND** the requester's configured scope for that capability is `home` or
  narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Cross-bundle list enumerates peer bundle under all-all scope

- **WHEN** a requester with `list = all` lists a configured peer bundle's
  sessions
- **THEN** relay returns the peer bundle's session listing rather than rejecting
  the requester as unknown

#### Scenario: Relay-wide principal needs all-all to reach a bundle

- **WHEN** a relay-wide principal (for example a `@GLOBAL` operator) issues a
  `list` or `send` targeting a bundle namespace
- **AND** its configured scope for that capability is `home`
- **THEN** relay returns `authorization_forbidden`, because the bundle is not the
  principal's home (`GLOBAL`) namespace
- **AND** the same principal under `all` is permitted

#### Scenario: Capability not configurable to cross-bundle scope fails uniformly

- **WHEN** a requester issues a cross-bundle request for a capability whose
  policy-schema cap is below `all`
- **THEN** the request fails the uniform `all` threshold with
  `authorization_forbidden`
- **AND** no operation-specific code override is involved

### Requirement: Transport Capability Contract

Every target reachable via look or raww SHALL have four transport capabilities,
derived at check time from its unified registry entry's `SessionType` rather than
stored as fields on the entry:

- `can_be_looked` — the session can be targeted by `look` (its transport
  supports snapshot capture)
- `can_be_written` — the session can be targeted by `raww` (its transport
  supports raw input injection)
- `can_stream_output` — the session's transport natively produces live
  output chunks (ACP and PTY stream output natively; Tmux requires periodic
  polling)
- `can_give_choices` — the session's transport can surface choice requests
  (the transport produces ACP-style option arrays for operator/UI resolution).
  Describes choice *production*, not resolution authority — any session with
  sufficient `choose` policy scope may resolve choices regardless of its own
  `can_give_choices` value.

Capabilities SHALL be derived from the entry's `SessionType` (at check time).
Bundle entries derive the type from bundle configuration at startup/reconcile;
relay-wide entries derive it from `users.toml` at startup for declared principals
(registered offline) or at Hello for dynamically-created principals. This makes
the registry entry the operation-time source of truth for target capabilities
instead of reloading different configuration sources for bundle and relay-wide
targets.

| Transport | `can_be_looked` | `can_be_written` | `can_stream_output` | `can_give_choices` |
|-----------|----------------|-----------------|--------------------|--------------------|
| `Tmux`    | true           | true            | false              | false              |
| `Acp`     | true           | true            | true               | true               |
| `Pty`     | true           | true            | true               | false              |
| `Ui`      | false          | false           | false              | false              |
| `Pubsub`  | false          | false           | false              | false              |

The `Pty` row is normative and forward-looking: no `Pty` session type exists in
the current implementation, but it remains the expected long-term replacement
for tmux-backed prompt injection.

`can_stream_output` is advertised on registration; streaming look semantics that
consume it are deferred to a follow-on proposal.

When a look or raww operation resolves a target whose entry-derived capability for
that operation is false, relay SHALL return `validation_unsupported_operation`.
This check precedes authorization policy checks and applies uniformly to bundle
targets and relay-wide targets.

#### Scenario: Reject look against session with can_be_looked false

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Reject raww against session with can_be_written false

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Permit look against session with can_be_looked true

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = true`
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: Permit raww against session with can_be_written true

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = true`
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: ACP session advertises can_give_choices true

- **WHEN** an ACP-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = true`

#### Scenario: Tmux session advertises can_give_choices false

- **WHEN** a Tmux-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = false`

## ADDED Requirements

### Requirement: Unified Namespace-Keyed Session Registry

The relay SHALL maintain one session registry keyed by canonical `principal_id`
(`session@namespace`). The registry SHALL NOT use separate key variants or maps
for bundle sessions and relay-wide sessions.

The registry SHALL hold **every known principal**: static entries for configured
principals — bundle sessions AND `users.toml`-declared relay-wide principals — and
dynamic stream state attached when a principal connects. Configured bundle members
SHALL be registered as static entries whenever the bundle is hosted, independent
of transport startup mode: autostart, process-only (`--no-autostart`), and
reconcile/`up` SHALL all register the bundle's members, so a configured principal
is present in the registry before any Hello regardless of whether its transport
has been started. `users.toml`-declared relay-wide principals SHALL be registered
at relay startup. **Offline is a state, not absence**: a configured (static) entry
MAY exist before its principal is ready or connected, and that state SHALL NOT be
treated as an unknown target. A principal with no static declaration (e.g. an
application/relay principal, or a dynamically-created principal) registers a
dynamic entry at Hello.

Each registry entry SHALL include:

- canonical `principal_id`
- bare session id
- namespace
- principal class
- registration source (`Configured` for static bundle/`users.toml` principals,
  `Stream` for dynamic-only connections)
- session type / transport binding — the source from which look/raww transport
  capabilities are derived at check time (the relay does not store duplicate
  capability-flag fields)
- stream writer and revoke signal when connected
- authenticated identity when present

The entry is a routing/capabilities record only. It SHALL NOT store
delivery-layer readiness state: readiness is owned exclusively by
`AsyncWorkerEntry` in the delivery layer. Likewise the coder runtime directory is
carried on the delivery route, not stored on the entry. Surfaces that need to
report readiness (e.g. a per-session ready flag in a `list` response) SHALL
resolve it from `AsyncWorkerEntry` at read time rather than storing a copy on the
registry entry, so there is a single authoritative source for that fact.

Bundle lifecycle, credential revocation, listing, event fan-out, and delivery
lookup SHALL operate by filtering or looking up entries in this unified registry.

#### Scenario: Register bundle-runtime session by canonical principal id

- **WHEN** bundle session `agent` in namespace `bundle-a` is loaded during
  runtime startup or reconcile
- **THEN** the registry entry key is `agent@bundle-a`
- **AND** the entry namespace is `bundle-a`
- **AND** the entry carries its transport binding and session type, from which
  look/raww capabilities are derived
- **AND** the entry does not store readiness state

#### Scenario: Register configured member under process-only hosting

- **WHEN** a bundle is hosted with `--no-autostart` (process-only), so no
  transport startup runs for its members
- **THEN** a static registry entry exists for each configured member keyed by its
  canonical `principal_id`, before any Hello
- **AND** a `look`/`raww`/`list` targeting such a member resolves it as a known
  (offline) principal rather than `validation_unknown_target`

#### Scenario: Register declared relay-wide principal offline at startup

- **WHEN** relay-wide principal `operator@GLOBAL` is declared in `users.toml`
- **THEN** a static registry entry keyed `operator@GLOBAL` exists at startup,
  before any connection (offline is a state, not absence)
- **AND** a `look`/`raww` targeting it resolves capability from that entry —
  `validation_unsupported_operation` for a `Ui` principal, not
  `validation_unknown_target`
- **AND** an undeclared relay-wide target absent from the registry yields
  `validation_unknown_target`

#### Scenario: Register GLOBAL session by canonical principal id

- **WHEN** relay-wide principal `operator@GLOBAL` sends a healthy Hello
- **THEN** the registry entry key is `operator@GLOBAL`
- **AND** the entry namespace is `GLOBAL`
- **AND** the Hello attaches dynamic stream writer/revoke state to the entry
  (flipping a declared principal from offline to online)

#### Scenario: Reject duplicate canonical registration

- **WHEN** a registry entry already exists for `agent@bundle-a`
- **AND** another connection tries to register the same canonical principal id
- **THEN** relay applies the existing identity-claim collision behavior

#### Scenario: Evict bundle namespace entries on bundle unload

- **WHEN** bundle `bundle-a` is unloaded or reloaded
- **THEN** relay evicts registry entries whose namespace is `bundle-a`
- **AND** entries in `GLOBAL` and other bundle namespaces remain registered

#### Scenario: Preserve configured-but-not-ready target behavior

- **WHEN** a configured coder target has a registry entry but its transport is not
  ready
- **THEN** target lookup does not return `validation_unknown_target`
- **AND** send, look, and raww preserve their existing unavailable, stale, queued,
  or transport-specific behavior for that readiness state

#### Scenario: Resolve per-session readiness from the delivery layer

- **WHEN** a `list` response needs to report whether a coder session is ready
- **THEN** the ready state is computed at list-generation time from the delivery
  worker registry (`AsyncWorkerEntry`)
- **AND** no readiness value is read from or stored on the unified registry entry

## REMOVED Requirements

### Requirement: Retire GLOBAL Routing Stub

**Reason**: Subsumed by the unified registry under the Suffix-Based Target
Routing requirement.
