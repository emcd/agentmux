# addressing-routing Specification

## Purpose

Canonical IDs, namespace semantics, target resolution, list payloads, raww target resolution.

## Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version` (supported value for this schema: `1`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - exactly one session shape: a coder-backed shape (a flat `coder` reference,
    with optional `coder-session-id`) or a coder-less shape (exactly one
    `[sessions.ui]` or `[sessions.pubsub]` marker subtable)

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present

A coder-backed `[[sessions]]` entry SHALL carry:

- required `coder` reference (must resolve to a `[[coders]]` entry)
- optional `coder-session-id`

The session's transport SHALL be derived from the referenced coder's
descriptor:

- `[coders.tmux]` → Tmux-backed coder delivery
- `[coders.acp]` → ACP-backed coder delivery
- `[coders.pty]` → Pty-backed coder delivery via libghostty-vt + portable-pty

The session entry SHALL NOT restate the transport; the coder descriptor is
authoritative.

A coder-less `[[sessions]]` entry SHALL declare exactly one `[sessions.ui]` or
`[sessions.pubsub]` marker subtable, which SHALL carry no required fields
(empty body is valid). A coder-less entry SHALL NOT carry a `coder` or
`coder-session-id` field.

Coder definitions SHALL include target descriptors in `coders.toml`:

- `format-version` (supported value for this schema: `1`)
- `[[coders]]` entries with:
  - `id`
  - exactly one target descriptor table:
    - `[coders.tmux]`
    - `[coders.acp]`
    - `[coders.pty]`

Descriptor fields SHALL be:

- `[coders.tmux]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
- `[coders.acp]`:
  - required `channel` (`stdio` | `http`)
  - for `channel = "stdio"`: required `command`
  - for `channel = "http"`: required `url`; optional `headers` entries
    (`name`, `value`)
- `[coders.pty]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
  - optional `cols` (default 120) and `rows` (default 40)
  - optional `term-protocol` (default `xterm-256color`)

This enumeration is authoritative for what an operator may write, so it SHALL be
kept complete as descriptor keys are added, and SHALL be reconciled against the
loader rather than extended only with the key a change happens to introduce.

**No per-coder descriptor carries a delivery timeout.** How long a delivery may
wait is a property of the relay's patience, not of any coder, and is configured
relay-side per the `runtime-bootstrap` capability's `Relay Configuration File`
requirement. The prompt-readiness keys above remain per-coder because a prompt
frame genuinely is a property of the coder.

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session and SHALL NOT
  silently fall back to `session/new`.

Pty and Tmux lifecycle selection constraints:

- if a coder-backed session includes `coder-session-id` and the coder defines
  `[coders.pty]` (Pty) or `[coders.tmux]` (Tmux), the runtime SHALL construct
  the resume command by substituting `{coder-session-id}` into the
  `resume-command` template.
- if the coder-backed session omits `coder-session-id`, the runtime SHALL
  use the `initial-command` template.
- if the template substitution leaves an unresolved placeholder, the
  validator SHALL reject the configuration during load.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid tmux-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.tmux]`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the tmux transport

#### Scenario: Load valid ACP-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference with
  `coder-session-id`
- **AND** the referenced coder defines `[coders.acp]` with `channel = "stdio"`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the ACP transport

#### Scenario: Load valid Pty-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.pty]` with
  `initial-command` and `resume-command`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the Pty transport
- **AND** the Pty transport spawns the child under a portable-pty master
  sized to the per-coder `cols` and `rows` (defaults 120 x 40)

#### Scenario: Reject session with neither coder nor marker

- **WHEN** a bundle session entry declares no `coder` reference and no
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject session declaring both coder and marker

- **WHEN** a bundle session entry declares a `coder` reference and also a
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject coder declaring both Pty and Tmux target descriptors

- **WHEN** a `[[coders]]` entry declares both `[coders.pty]` and
  `[coders.tmux]` subtables
- **THEN** the validator rejects the configuration with a structured config
  error

### Requirement: Bundle Group Membership Field

Per-bundle TOML configuration SHALL support optional top-level bundle group
membership field:

- `groups` (`string[]`)

This field applies to bundle lifecycle command grouping (`up/down`) and SHALL
NOT change session routing identity semantics.

Group naming rules:

- reserved/system group names are uppercase
- custom group names are lowercase
- `ALL` is reserved and implicit

#### Scenario: Accept bundle file with custom groups

- **WHEN** bundle file includes `groups = ["dev", "login"]`
- **THEN** the system loads the bundle configuration successfully

#### Scenario: Accept bundle file without groups

- **WHEN** bundle file omits `groups`
- **THEN** the system loads the bundle configuration successfully

#### Scenario: Reject explicit ALL group in bundle groups

- **WHEN** bundle file includes `ALL` in `groups`
- **THEN** the system rejects configuration with
  `validation_reserved_group_name`

#### Scenario: Reject invalid uppercase custom group

- **WHEN** bundle file includes uppercase custom group name not reserved by
  system
- **THEN** the system rejects configuration with
  `validation_invalid_group_name`

### Requirement: Session Routing Primitive

The system SHALL expose session ids as the routing primitive for message
delivery.
The system SHALL resolve each target session to its delivery endpoint at
delivery time using session type from config:

- `tmux` sessions: prompt-injection/quiescence delivery path
- `acp` sessions: ACP worker delivery path
- `pty` sessions: native PTY delivery path via libghostty-vt + portable-pty
  (new in `add-pty-transport`)
- `ui` sessions: stream push event delivery path
- `pubsub` sessions: embedded callback delivery path

The system SHALL support directed delivery to one or more explicitly selected
target sessions.

For send explicit targets, relay SHALL accept only canonical target identifiers
in `session@bundle` form or bare `session_id` values that resolve unambiguously
within the sending bundle.

Relay SHALL NOT resolve configured bundle session `name` values as send-target
aliases.
Session `name` remains informational metadata only and is not send-routable.

#### Scenario: Resolve session target for direct send by session type

- **WHEN** a caller sends a message to target `session_id`
- **THEN** the system routes to that session using its configured session type
- **AND** resolves the appropriate delivery endpoint for that type
- **AND** the resolution distinguishes `tmux`, `acp`, `pty`, `ui`, and `pubsub`
  delivery endpoints

#### Scenario: Reject configured name alias as explicit send target

- **WHEN** a caller sends a message to a configured `name` alias rather than
  the canonical `session_id`
- **THEN** the relay rejects the request with a validation error

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

### Requirement: Request Routing Namespace

Request frames on a registered stream SHALL carry an optional `namespace` field
(formerly `bundle_name`) on the request envelope. The relay SHALL resolve the
routing context for the request as follows:

- `namespace` present, value is a bundle name → route to that bundle via
  catalog lookup, regardless of any connection binding.
- `namespace` absent + connection is bundle-bound (session principal) → route
  to the connection's bound bundle.
- `namespace` absent + connection is relay-wide (non-session principal) → relay
  SHALL return a typed error (`validation_missing_routing_namespace`).

The relay SHALL reject client-supplied `namespace` values of `"EXTERNAL"` or
`"RELAY"` with `validation_unsupported_namespace`; these are reserved for
relay-internal routing only. Routing to `"GLOBAL"` and other relay-wide
targets via target principal ID suffix inference is specified in
`add-global-namespace-routing`.

#### Scenario: Explicit bundle namespace routes to bundle

- **WHEN** a registered stream submits a request with `namespace = "agentmux"`
- **THEN** relay routes the request in the context of bundle `agentmux`
- **AND** targets are resolved against bundle `agentmux` members

#### Scenario: Absent namespace uses bound bundle

- **WHEN** a session principal stream submits a request without `namespace`
- **THEN** relay routes the request in the context of the connection's bound bundle

#### Scenario: Absent namespace on relay-wide connection returns error

- **WHEN** a relay-wide principal stream submits a request without `namespace`
- **THEN** relay returns `validation_missing_routing_namespace`

#### Scenario: EXTERNAL and RELAY namespaces are rejected

- **WHEN** a client submits a request with `namespace = "EXTERNAL"` or
  `namespace = "RELAY"`
- **THEN** relay returns `validation_unsupported_namespace`

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

### Requirement: Canonical Session Identity

All relay internal state and wire-facing output SHALL represent session
identity in `session@bundle` canonical form.

Canonical identity SHALL be hydrated at `hello` registration:
`{session_id}@{bundle_name}`. The hydrated form SHALL be used for all
subsequent operations on that stream and in all relay responses and events.

Wire fields carrying session identity (`target_session`, `requester_session`,
`session_id` in listing responses, `decided_by` in decision responses) SHALL
emit the canonical form.

Global users (from `users.toml`) carry `@GLOBAL` in their `session_id`;
their canonical form is their configured `id` unchanged.

#### Scenario: Emit canonical requester identity in send response

- **WHEN** a session with `session_id = "master"` in bundle `"agentmux"` sends
  a message
- **THEN** relay send response includes `requester_session = "master@agentmux"`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`

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

### Requirement: Verified Identity Trust Boundary

The system SHALL enforce a same-host, same-user socket trust boundary as the
access prerequisite. All connecting principals SHALL present an `identity_token`
in the Hello frame (see: `relay-identity` — Verifiable Session Identity).

When a session credential is verified and a `principal_id` is assigned, the
`principal_id` SHALL be the authoritative identity for authorization decisions.
Session connections that send the `"socket-trust"` placeholder operate as
socket-trusted participants with no authenticated principal; in this mode the
relay SHALL fall back to association/socket-driven requester identity, the
same baseline as before identity federation.

Caller-supplied sender-like payload fields SHALL NOT override principal
identity in either mode.

The relay SHALL operate against tmux and relay resources owned by the current
host user. This scope does not change.

#### Scenario: Operate against current user's tmux server

- **WHEN** delivery or reconciliation executes
- **THEN** the system targets tmux resources owned by the current host user

#### Scenario: Verified principal takes precedence over self-asserted session_id

- **WHEN** a session has completed credential verification and holds a
  `principal_id`
- **THEN** relay authorization decisions use the verified `principal_id`
- **AND** self-asserted `session_id` values do not influence principal identity

#### Scenario: Socket-trusted session falls back to requester identity

- **WHEN** a session connects with `identity_token = "socket-trust"`
- **AND** `require_session_credentials = false`
- **THEN** the relay authorizes the session using association/socket-driven
  requester identity
- **AND** the session is not assigned a `principal_id`

#### Scenario: Caller-supplied sender override rejected

- **WHEN** a caller supplies a sender-like payload field that conflicts with
  the established principal or requester identity
- **THEN** the relay authorizes against the established identity
- **AND** does not treat the payload field as authoritative

### Requirement: Session Type Taxonomy

The relay SHALL recognize exactly five session types, resolved from config:

| Type | Origin | Delivery binding | Notes |
|---|---|---|---|
| `tmux` | coder-backed; coder defines `[coders.tmux]` | tmux pane prompt injection + quiescence gating | MCP server socket; request/reply |
| `acp` | coder-backed; coder defines `[coders.acp]` | ACP prompt via relay-spawned worker | Bidirectional; relay drives channel |
| `pty` | coder-backed; coder defines `[coders.pty]` | native PTY write via libghostty-vt + portable-pty | Relay owns the child and its terminal |
| `ui` | coder-less `[sessions.ui]` marker | live relay stream push events | Bare marker subtable; no required fields |
| `pubsub` | coder-less `[sessions.pubsub]` marker | embedded callback; envelope as prompt | In-process tool calls |

A coder-backed session's type (`tmux`, `acp`, or `pty`) SHALL be derived from
the referenced coder's descriptor; the session entry SHALL NOT restate it. A
coder-less session's type (`ui` or `pubsub`) SHALL be its declared marker
subtable.

Session type SHALL be determined solely from config. Hello frames SHALL NOT
carry or assert session type.

`ui` and `pubsub` session types SHALL be recognized and validated from day one.
Sessions of these types SHALL be excluded from active routing at startup with a
structured `runtime_session_type_not_implemented` failure rather than a parse
error, until delivery is implemented.

#### Scenario: Derive tmux session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.tmux]`
- **AND** the relay starts up
- **THEN** relay routes messages to that session via prompt injection

#### Scenario: Derive acp session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.acp]`
- **THEN** relay delivers to that session via the ACP worker path

#### Scenario: Derive pty session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.pty]`
- **THEN** relay delivers to that session via the Pty transport

#### Scenario: Fail fast for unimplemented session type

- **WHEN** a session entry declares a `[sessions.ui]` or `[sessions.pubsub]`
  marker subtable
- **THEN** relay emits `runtime_session_type_not_implemented` for that session
- **AND** excludes it from routing without aborting other session startup

### Requirement: Per-Session Readiness In List Payload

Relay list payloads SHALL include a required field `ready: bool` on each
`ListedSession` entry.

`ready` SHALL be derived on each list request from a per-transport
readiness predicate:

- tmux member: `ready=true` iff relay resolves an active pane target for
  the configured tmux session.
- ACP member: `ready=true` iff the shared per-target ACP worker reports
  ready state for the configured ACP session.
- ui or pubsub member: `ready=false` always (no implemented startup path).

Per-session readiness SHALL be the single source of truth used to derive
the bundle-level aggregates (`state`, `startup_health`, `hosted`) within
the same list request.

#### Scenario: Report ready true for tmux session with resolvable pane

- **WHEN** a configured tmux member has a resolvable active pane target
- **THEN** the listed session entry reports `ready=true`

#### Scenario: Report ready false for tmux session without resolvable pane

- **WHEN** a configured tmux member has no resolvable active pane target
- **THEN** the listed session entry reports `ready=false`

#### Scenario: Report ready true for ACP session with ready worker

- **WHEN** a configured ACP member has a ready shared worker
- **THEN** the listed session entry reports `ready=true`

#### Scenario: Report ready false for ACP session without ready worker

- **WHEN** a configured ACP member has no ready shared worker
- **THEN** the listed session entry reports `ready=false`

#### Scenario: Report ready false for ui or pubsub member

- **WHEN** a configured member is of transport ui or pubsub
- **THEN** the listed session entry reports `ready=false`

### Requirement: Bundle Hosted Flag In List Payload

Relay list payloads SHALL include a required field `hosted: bool` on the
canonical `ListedBundle` payload.

`hosted` SHALL be derived on each list request from per-session readiness
and SHALL be independent of `state`, `startup_health`, and
`state_reason_code`.

Hosting predicate:

- `hosted=true` iff at least one configured member is ready.
- `hosted=false` otherwise, including the empty-bundle case
  (zero configured members).

`hosted` SHALL NOT alter or replace existing `state` (`up|down`) or
`startup_health` semantics. `state_reason_code` SHALL continue to describe
`state` and SHALL NOT be suppressed when `hosted=false`.

#### Scenario: Report hosted true when at least one member is ready

- **WHEN** at least one configured bundle member is ready
- **THEN** relay reports `hosted=true`

#### Scenario: Report hosted false when no configured member is ready

- **WHEN** zero configured bundle members are ready
- **THEN** relay reports `hosted=false`

#### Scenario: Report hosted false for ACP-only bundle with no ready worker

- **WHEN** the bundle has only configured ACP members
- **AND** none of those ACP members report ready
- **THEN** relay reports `hosted=false`

#### Scenario: Preserve state and reason fields when hosted false

- **WHEN** relay reports `hosted=false`
- **AND** zero configured sessions are currently ready
- **THEN** relay reports `state=down`
- **AND** `state_reason_code` continues to describe the down condition

### Requirement: Relay raww target resolution and bundle boundary

Raww targets SHALL be resolved using the shared single-target routing stage.

Validation behavior:

- bare/unqualified target (no `@<namespace>` suffix) →
  `validation_unqualified_target`
- reserved namespace (`@EXTERNAL`/`@RELAY`) target →
  `validation_unsupported_namespace`
- unknown/non-canonical target → `validation_unknown_target`
- resolved target with `can_be_written = false` →
  `validation_unsupported_operation` (see Transport Capability Contract)
- cross-bundle raww with insufficient scope → `authorization_forbidden`

Relay-wide (`@GLOBAL`) targets are no longer rejected at the routing stage;
rejection occurs at the capability check using `validation_unsupported_operation`
when the resolved session carries `can_be_written = false`. This separates namespace
routing from operation-capability concerns.

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then capability, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage; their reserved namespace target rejection is
uniform.

After this change, the routing stage for look and raww SHALL resolve `@GLOBAL`
targets as relay-wide rather than rejecting them at the routing stage; the
handler then derives the resolved target's session type and applies the
capability check. The `RelayWideTargets` enum and `resolve_target`'s
relay-wide-targets parameter are removed in this change — dead code once the
single `Rejected` call site is gone.

#### Scenario: Reject unqualified raww target

- **WHEN** caller invokes `raww` with a target without `@<namespace>` suffix
- **THEN** relay returns `validation_unqualified_target`

#### Scenario: Reject reserved namespace raww target

- **WHEN** caller invokes `raww` with an `@EXTERNAL` or `@RELAY` target
- **THEN** relay returns `validation_unsupported_namespace`

#### Scenario: Reject relay-wide raww target via capability check

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target
- **THEN** relay returns `validation_unsupported_operation`
- **AND** the rejection is uniform with the look capability check for the
  same target

#### Scenario: Cross-bundle raww denied by scope

- **WHEN** caller invokes `raww` with a target in a different bundle
- **AND** requester's `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`
