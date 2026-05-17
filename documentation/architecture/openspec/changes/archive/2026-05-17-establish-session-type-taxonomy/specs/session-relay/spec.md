## REMOVED Requirements

### Requirement: Permission Decision Submitter Gate
**Reason**: Permission-decision authority is expressed entirely by the
`grant` policy capability. Session class is not a meaningful
authorization axis and is eliminated by the session-type taxonomy.
**Migration**: Remove the submitter-class gate. Evaluate `permission.resolve`
authority via `grant` alone.

### Requirement: Endpoint Class Routing Behavior
**Reason**: Routing is derived from session type declared in config, not from
`client_class` asserted at connect time. Replaced by Session Type Taxonomy.
**Migration**: See Session Type Taxonomy requirement.

## MODIFIED Requirements

### Requirement: Hello Registration Contract

Each client stream SHALL begin with a `hello` registration frame containing:

- `bundle_name`
- `session_id`
- `schema_version`

`hello` SHALL carry identity only. No transport class, mode, or privilege field
is accepted; relay SHALL reject unrecognized fields.

The relay SHALL hydrate canonical identity as `{session_id}@{bundle_name}` at
registration. All subsequent internal state and wire output SHALL use the
canonical form.

Identity lookup at hello registration SHALL proceed in order:

1. Bundle `[[sessions]]` for the named `bundle_name` (bundle members)
2. Global users from `users.toml` when `session_id` carries a `@GLOBAL` suffix

If no match is found, relay SHALL reject with `validation_unknown_sender`.

If a second stream attempts `hello` for the same canonical identity while the
current owner is live, relay SHALL reject the second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for configured bundle member

- **WHEN** a client sends valid `hello` with `bundle_name = "agentmux"` and
  `session_id = "master"`
- **AND** `session_id` maps to a configured bundle member
- **THEN** relay accepts hello and binds stream to canonical identity
  `master@agentmux`

#### Scenario: Accept hello for configured global user

- **WHEN** a client sends valid `hello` with `session_id = "user@GLOBAL"`
- **AND** `session_id` matches a configured global user entry in `users.toml`
- **THEN** relay accepts hello and binds stream using that canonical identity

#### Scenario: Reject hello for unknown session

- **WHEN** a client sends `hello` with a `session_id` not present in bundle
  members or (for `@GLOBAL` suffix) in `users.toml`
- **THEN** relay rejects with `validation_unknown_sender`

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

The session's transport (tmux pane injection vs. ACP worker delivery) SHALL be
derived from the referenced coder's descriptor (`[coders.tmux]` vs.
`[coders.acp]`); the session entry SHALL NOT restate the transport.

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

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session and SHALL NOT
  silently fall back to `session/new`.

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

#### Scenario: Reject session with neither coder nor marker

- **WHEN** a bundle session entry declares no `coder` reference and no
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject session declaring both coder and marker

- **WHEN** a bundle session entry declares a `coder` reference and also a
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

### Requirement: Session Routing Primitive

The system SHALL expose session ids as the routing primitive for message
delivery.
The system SHALL resolve each target session to its delivery endpoint at
delivery time using session type from config:

- `tmux` sessions: prompt-injection/quiescence delivery path
- `acp` sessions: ACP worker delivery path
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

#### Scenario: Reject configured name alias as explicit send target

- **WHEN** a caller sends using a session `name` value as explicit target
- **THEN** relay rejects the target as unresolvable

### Requirement: Non-Spoofable Decision Actor Identity

Relay SHALL derive permission decision actor identity from the authenticated
stream context and SHALL NOT trust caller-supplied identity fields in the
action payload.

#### Scenario: Reject caller-supplied decision actor field

- **WHEN** `permission.resolve` payload includes any caller-supplied identity
  field (e.g., `decided_by`, `session_id`, or similar)
- **THEN** relay rejects with `validation_invalid_params`

## ADDED Requirements

### Requirement: Session Type Taxonomy

The relay SHALL recognize exactly four session types, resolved from config:

| Type | Origin | Delivery binding | Notes |
|---|---|---|---|
| `tmux` | coder-backed; coder defines `[coders.tmux]` | tmux pane prompt injection + quiescence gating | MCP server socket; request/reply |
| `acp` | coder-backed; coder defines `[coders.acp]` | ACP prompt via relay-spawned worker | Bidirectional; relay drives channel |
| `ui` | coder-less `[sessions.ui]` marker | live relay stream push events | Bare marker subtable; no required fields |
| `pubsub` | coder-less `[sessions.pubsub]` marker | embedded callback; envelope as prompt | In-process tool calls |

A coder-backed session's type (`tmux` or `acp`) SHALL be derived from the
referenced coder's descriptor; the session entry SHALL NOT restate it. A
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

#### Scenario: Fail fast for unimplemented session type

- **WHEN** a session entry declares a `[sessions.ui]` or `[sessions.pubsub]`
  marker subtable
- **THEN** relay emits `runtime_session_type_not_implemented` for that session
- **AND** excludes it from routing without aborting other session startup

### Requirement: Canonical Session Identity

All relay internal state and wire-facing output SHALL represent session
identity in `session@bundle` canonical form.

Canonical identity SHALL be hydrated at `hello` registration:
`{session_id}@{bundle_name}`. The hydrated form SHALL be used for all
subsequent operations on that stream and in all relay responses and events.

Wire fields carrying session identity (`target_session`, `sender_session`,
`session_id` in listing responses, `decided_by` in decision responses) SHALL
emit the canonical form.

Global users (from `users.toml`) carry `@GLOBAL` in their `session_id`;
their canonical form is their configured `id` unchanged.

#### Scenario: Emit canonical sender identity in send response

- **WHEN** a session with `session_id = "master"` in bundle `"agentmux"` sends
  a message
- **THEN** relay send response includes `sender_session = "master@agentmux"`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`
