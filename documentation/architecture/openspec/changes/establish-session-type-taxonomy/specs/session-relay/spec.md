## REMOVED Requirements

### Requirement: UI-Mediated Decision Submitter Gate
**Reason**: Permission-decision authority is expressed entirely by the
`authorize_grant` policy capability. Session class is not a meaningful
authorization axis and is eliminated by the session-type taxonomy.
**Migration**: Remove the submitter-class gate. Evaluate `permission.resolve`
authority via `authorize_grant` alone.

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

- `format-version` (supported value for this schema: `2`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - exactly one session-type subtable from the closed set
    `{[sessions.tmux], [sessions.acp], [sessions.ui], [sessions.pubsub]}`

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present

`[sessions.tmux]` subtable fields SHALL be:

- required `coder` reference (must resolve to a `[coders.tmux]` coder)
- optional `coder-session-id`

`[sessions.acp]` subtable fields SHALL be:

- required `coder` reference (must resolve to a `[coders.acp]` coder)
- optional `coder-session-id`

`[sessions.ui]` and `[sessions.pubsub]` subtables SHALL carry no required
fields (empty body is valid).

Coder definitions SHALL include target descriptors in `coders.toml`:

- `format-version` (supported value for this schema: `2`)
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

#### Scenario: Load valid v2 tmux-type session configuration

- **WHEN** bundle file uses `format-version = 2`
- **AND** a session entry declares `[sessions.tmux]` with a valid coder
  reference
- **AND** the referenced coder defines `[coders.tmux]`
- **THEN** the system loads configuration successfully

#### Scenario: Load valid v2 ACP-type session configuration

- **WHEN** bundle and coders files use `format-version = 2`
- **AND** a session entry declares `[sessions.acp]` with `coder-session-id`
- **AND** the referenced coder defines `[coders.acp]` with `channel = "stdio"`
- **THEN** the system loads configuration successfully

#### Scenario: Reject session with no session-type subtable

- **WHEN** a bundle session entry has no `[sessions.tmux]`, `[sessions.acp]`,
  `[sessions.ui]`, or `[sessions.pubsub]` subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject session with multiple session-type subtables

- **WHEN** a bundle session entry declares more than one session-type subtable
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

The relay SHALL recognize exactly four session types, declared by subtable in
each `[[sessions]]` config entry:

| Type | Delivery binding | Notes |
|---|---|---|
| `tmux` | tmux pane prompt injection + quiescence gating | MCP server socket; request/reply |
| `acp` | ACP prompt via relay-spawned worker | Bidirectional; relay drives channel |
| `ui` | live relay stream push events | Bare marker subtable; no required fields |
| `pubsub` | embedded callback; envelope as prompt | In-process tool calls |

Session type SHALL be determined solely from config. Hello frames SHALL NOT
carry or assert session type.

`ui` and `pubsub` session types SHALL be recognized and validated from day one.
Sessions of these types SHALL be excluded from active routing at startup with a
structured `runtime_session_type_not_implemented` failure rather than a parse
error, until delivery is implemented.

#### Scenario: Recognize tmux session type from config

- **WHEN** a session entry declares `[sessions.tmux]`
- **AND** the relay starts up
- **THEN** relay routes messages to that session via prompt injection

#### Scenario: Recognize acp session type from config

- **WHEN** a session entry declares `[sessions.acp]`
- **THEN** relay delivers to that session via the ACP worker path

#### Scenario: Fail fast for unimplemented session type

- **WHEN** a session entry declares `[sessions.ui]` or `[sessions.pubsub]`
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

### Requirement: Session-Coder Type Consistency

The relay SHALL enforce type consistency between session and coder at config
load time:

- A `[sessions.tmux]` entry SHALL reference a coder with a `[coders.tmux]`
  descriptor.
- A `[sessions.acp]` entry SHALL reference a coder with a `[coders.acp]`
  descriptor.
- `[sessions.ui]` and `[sessions.pubsub]` entries SHALL carry no coder
  reference; a coder reference on these types is a config error.

Mismatches SHALL fail fast with a structured config validation error.

#### Scenario: Reject tmux session with acp coder

- **WHEN** a session entry declares `[sessions.tmux]`
- **AND** the referenced coder defines `[coders.acp]`
- **THEN** relay rejects configuration with a structured session-coder type
  mismatch error

#### Scenario: Reject ui session with any coder reference

- **WHEN** a session entry declares `[sessions.ui]`
- **AND** the entry includes a `coder` field
- **THEN** relay rejects configuration with a structured config error
