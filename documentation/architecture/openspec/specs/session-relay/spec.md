# session-relay Specification

## Purpose
The core relay operation specifications — the largest live capability and the destination for the majority of archived changes. The spec governs bundle membership configuration, reconciliation lifecycle and startup failure visibility, session routing primitives with canonical `session@bundle` identity, and the send/look/raww/choose operation contracts with their per-target response shapes. The ACP worker lifecycle and shared replay buffer (1000-entry cap with snapshot/cursor accessors), ACP permission/choice queue (bounded at `pending_max` default 256 with `choices.snapshot`/`requested`/`resolved` event carrier), policy vocabulary and authorization evaluation order, session type taxonomy (tmux/acp/ui/pubsub), persistent relay client streams, dynamic bundle file watching, and `SessionType` transport capability flags are also normative.
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

### Requirement: Bundle Reconciliation

The system SHALL provide a reconciliation operation that ensures all known
bundle sessions are online under the same host user.

#### Scenario: Start missing session during reconciliation

- **WHEN** reconciliation runs and a configured session is absent
- **THEN** the system creates that tmux session
- **AND** starts the configured coder command in the configured working
  directory

#### Scenario: Keep existing session during reconciliation

- **WHEN** reconciliation runs and a configured session already exists
- **THEN** the system leaves that session running

#### Scenario: Reconciliation does not depend on start-server only

- **WHEN** reconciliation needs to bring a missing session online
- **THEN** the system creates the session directly
- **AND** does not treat `tmux start-server` alone as sufficient readiness

### Requirement: Reconciliation Lifecycle Policy

The system SHALL implement startup and cleanup behavior that minimizes session
creation races and avoids leaking idle tmux servers.

#### Scenario: Bootstrap then parallel session creation

- **WHEN** multiple configured sessions are missing during reconciliation
- **THEN** the system creates one deterministic bootstrap session first
- **AND** creates remaining missing sessions in parallel after bootstrap

#### Scenario: Retry transient creation races

- **WHEN** session creation fails with a transient tmux readiness error
- **THEN** the system retries with bounded attempts
- **AND** applies short jitter between retries

#### Scenario: Track agentmux-owned sessions

- **WHEN** the system creates a session during reconciliation
- **THEN** the system marks that session as agentmux-owned using tmux metadata

#### Scenario: Cleanup dedicated socket server only when fully idle

- **WHEN** reconciliation or pruning finds zero agentmux-owned sessions on a
  dedicated configured socket and zero total sessions remain on that socket
- **THEN** the system shuts down that socket's tmux server
- **AND** does not require `exit-empty` to be turned off for startup

#### Scenario: Preserve socket server while non-owned sessions exist

- **WHEN** reconciliation or pruning finds zero agentmux-owned sessions on a
  dedicated configured socket but non-owned sessions remain
- **THEN** the system does not shut down that socket's tmux server

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

### Requirement: JSON Send Envelope

The system SHALL inject messages as strict, pretty-printed JSON envelopes.

Each envelope SHALL include:

- `schema_version`
- `message_id` (globally unique identifier)
- `sender_session`
- `target_session` or broadcast marker
- `created_at`
- `body`

#### Scenario: Inject valid envelope

- **WHEN** a send request is accepted for delivery
- **THEN** the system renders a strict, pretty-printed JSON envelope
- **AND** injects the envelope into the target session via tmux

#### Scenario: Reject malformed envelope input fields

- **WHEN** required message fields are missing or invalid
- **THEN** the system rejects the request with a validation error

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing.

For async delivery, relay SHALL keep accepted targets pending and wait for
quiescence before injection.

When request-level `quiescence_timeout_ms` is provided, relay SHALL use that
value as the wait bound for the delivery attempt.

Request-level `quiescence_timeout_ms` SHALL map to relay's effective delivery
wait timeout for the request.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the system injects the pending message

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** pane output continues changing
- **THEN** the system keeps the target pending
- **AND** attempts injection after a future quiescent window is observed

#### Scenario: Apply request quiescence timeout override

- **WHEN** request provides `quiescence_timeout_ms`
- **AND** no quiescent window is observed before that timeout
- **THEN** the system drops that pending target
- **AND** records timeout in relay diagnostics/inscriptions

#### Scenario: Map request timeout to relay delivery wait bound

- **WHEN** a request includes `quiescence_timeout_ms`
- **THEN** relay uses that value as the effective delivery wait timeout for the
  request

### Requirement: Quiescence Documentation

The system SHALL document quiescence constraints and known interference
patterns for users configuring agent sessions.

#### Scenario: Document dynamic output caveat

- **WHEN** project documentation is generated for the relay capability
- **THEN** it includes a warning that continuously changing output sources
  (for example clock-style statusline content) can prevent quiescence
  detection from succeeding

### Requirement: Delivery Results Without ACK Protocol

Relay SHALL use asynchronous acceptance responses and SHALL NOT support
synchronous completion responses.

An accepted send request SHALL return immediately with per-target `outcome =
queued`. Relay SHALL NOT block the caller waiting for delivery completion.

#### Scenario: Report accepted async delivery

- **WHEN** relay accepts a send request for one or more targets
- **THEN** the immediate result marks those targets as `queued`
- **AND** does not wait for final delivery outcome before responding

#### Scenario: Return no-op completion for zero effective targets

- **WHEN** sender exclusion and target resolution produce zero effective
  recipients
- **THEN** relay returns an immediate no-op response without validation error
- **AND** response contains zero per-target results

### Requirement: Configurable tmux socket

The system SHALL derive the tmux socket path for all tmux operations from the
configured state root and bundle name.

#### Scenario: Derive socket from default runtime roots

- **WHEN** no runtime root overrides are provided
- **THEN** the system uses the bundle runtime socket path under the default
  state root

#### Scenario: Derive socket from explicit runtime state root

- **WHEN** an explicit runtime state root is configured
- **THEN** the system uses that derived bundle socket path for session checks, reconciliation,
  pane capture, and message injection

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before relay injection.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output.

When `input_idle_cursor_column` is configured, relay SHALL treat the target as
prompt-ready only when tmux reports `cursor_x` at that configured column.

#### Scenario: Deliver when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay injects the message

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** relay treats target as prompt-ready

#### Scenario: Require idle input column before injection

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** tmux-reported `cursor_x` equals configured
  `input_idle_cursor_column`
- **THEN** relay injects the message

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** tmux-reported `cursor_x` differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not inject the message
- **AND** relay continues waiting until timeout

#### Scenario: Time out when quiescent pane never becomes prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** pane output reaches quiescence
- **AND** template matching conditions do not become true before delivery
  timeout
- **THEN** relay reports delivery timeout with prompt-readiness mismatch reason
- **AND** relay does not inject the message

### Requirement: Prompt-Readiness Template Validation

The system SHALL validate prompt-readiness template regex during bundle
configuration loading.

#### Scenario: Reject invalid prompt regex

- **WHEN** bundle configuration includes a malformed `prompt_regex`
- **THEN** bundle loading fails with a structured configuration validation
  error

### Requirement: Coder Command Template Resolution

The system SHALL resolve per-session startup commands from referenced coder
templates in `coders.toml`.

Each coder definition SHALL include:

- `id`
- `initial-command`
- `resume-command`
- optional `prompt-regex`
- optional `prompt-inspect-lines`
- optional `prompt-idle-column`

Resolution SHALL follow:

1. If session `coder-session-id` is set, use coder `resume-command`.
2. Otherwise use coder `initial-command`.

Template placeholders SHALL be validated before reconciliation starts. Unknown
or unresolved placeholders SHALL fail configuration validation.

#### Scenario: Use resume command when coder-session-id is present

- **WHEN** a session includes `coder-session-id`
- **THEN** the system resolves startup command from coder `resume-command`
- **AND** substitutes `{coder-session-id}` with the session value

#### Scenario: Use initial command when coder-session-id is absent

- **WHEN** a session does not include `coder-session-id`
- **THEN** the system resolves startup command from coder `initial-command`

#### Scenario: Reject unresolved placeholder during validation

- **WHEN** a chosen command template requires placeholders not provided by the
  session definition
- **THEN** the system rejects configuration with a validation error

### Requirement: Coder-Scoped Prompt-Readiness Templates

The system SHALL allow prompt-readiness templates to be defined per coder.
Sessions that reference a coder inherit that coder's prompt-readiness settings.

#### Scenario: Apply prompt regex from referenced coder

- **WHEN** a session references a coder that defines `prompt-regex`
- **THEN** relay evaluates prompt readiness for that session using the coder
  template

#### Scenario: Use coder prompt inspect line setting when configured

- **WHEN** a coder defines `prompt-inspect-lines`
- **THEN** relay uses that value as the prompt-readiness inspection window for
  sessions that reference the coder

#### Scenario: Use coder prompt idle column when configured

- **WHEN** a coder defines `prompt-idle-column`
- **THEN** relay requires tmux `cursor_x` to match that value before injection
  for sessions that reference the coder

### Requirement: Async Queue Lifecycle and Ordering

For `delivery_mode=async`, relay SHALL maintain an in-memory pending queue.
The queue SHALL be non-durable in MVP.
Relay SHALL preserve FIFO ordering per target session and SHALL NOT deduplicate
or coalesce queued messages.

#### Scenario: Drop pending async queue on relay restart

- **WHEN** relay exits or restarts before delivering queued async targets
- **THEN** pending async entries are discarded
- **AND** they are not recovered from durable storage in MVP

#### Scenario: Preserve per-target FIFO ordering

- **WHEN** multiple async messages are queued for the same target session
- **THEN** relay attempts delivery in enqueue order for that target

#### Scenario: Do not deduplicate queued async messages

- **WHEN** queued async messages have identical body content or same target set
- **THEN** relay treats them as distinct queue entries
- **AND** attempts each entry independently

### Requirement: Async Delivery Observability

Relay SHALL emit inscriptions for async queue lifecycle transitions.

#### Scenario: Record queued async acceptance

- **WHEN** relay accepts an async target for queued delivery
- **THEN** relay writes an inscription event containing target session and
  message id with queued state

#### Scenario: Record terminal async outcome

- **WHEN** an async queued target reaches a terminal state (`delivered`,
  `timeout`, or dropped on shutdown)
- **THEN** relay writes an inscription event containing target session,
  message id, and terminal outcome

### Requirement: Async Queue Growth Risk Disclosure

The system SHALL document that MVP async queueing has no built-in hard cap and
may grow without bound if targets never become ready.

#### Scenario: Document unbounded queue risk for operators

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it includes explicit guidance on unbounded pending queue risk
- **AND** suggests using `quiescence_timeout_ms` where bounded waits are needed

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

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required) — MAY be a bare session id (resolved within the
  requester's bound/dispatch bundle) or a peer-qualified `<session>@<bundle>`
  id that selects a peer bundle by suffix (consistent with `Send` target
  routing)
- `lines` (optional)
- `offset` (optional; default `0`) — for ACP targets, pages the entry
  window backward from the newest end; for tmux targets only `0` is valid
- `bundle_name` (optional, redundant) — treated as the dispatch-bundle echo; it
  SHALL NOT select or reject a peer bundle. The peer bundle is derived solely
  from the `target_session` suffix

The relay SHALL resolve the look target's hosting bundle from the
`target_session` suffix and SHALL capture the snapshot from that bundle's
runtime context.

Authorization posture for `look` SHALL be:

- default scope `self`
- self-inspection (requester equals target) is always permitted; this shortcut
  applies to same-bundle look only
- same-bundle inspection of a different session requires `home`
- cross-bundle inspection of a peer bundle's session requires `all`,
  evaluated against the requester's own (dispatch) bundle policy; `home`
  confers no authority beyond the requester's own bundle

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request target is bare and omits `bundle_name`
- **THEN** relay resolves the target within the associated (bound) bundle

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching the associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Resolve peer bundle from target suffix

- **WHEN** look request target is `<session>@<peer-bundle>` where
  `<peer-bundle>` differs from the requester's dispatch bundle
- **AND** `<peer-bundle>` is configured on the relay and `<session>` is a member
- **AND** requester policy has `look = "all"`
- **THEN** relay captures the snapshot from `<peer-bundle>`'s runtime and
  returns `bundle_name = <peer-bundle>` with the requester echoed in its own
  dispatch bundle

#### Scenario: Reject unknown peer bundle

- **WHEN** look request target names a bundle that is not configured on this
  relay
- **THEN** relay rejects request with `validation_unknown_bundle`

#### Scenario: Reject unknown peer session

- **WHEN** look request target names a configured peer bundle but a session that
  is not a member of that bundle
- **THEN** relay rejects request with `validation_unknown_target`

#### Scenario: Deny cross-bundle look under home scope

- **WHEN** requester targets a session in a peer bundle
- **AND** requester policy has `look = "home"` or narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Reject nonzero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is present and not equal to `0`
- **THEN** relay rejects request with `validation_offset_unsupported`

#### Scenario: Accept zero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is omitted or equal to `0`
- **THEN** relay accepts request and proceeds with the look operation

### Requirement: Look Capture Window Bounds

Look capture window SHALL use deterministic bounds, with the default
keyed on target type:

- default `lines = 120` for tmux targets
- default `lines = 50` for ACP targets
- maximum `lines = 1000`
- valid range `1..=1000`

For ACP targets, the entry window SHALL be selected from the newest end of
the available buffer using `offset`:

- the window is the half-open range `[start, end)` over the full ordered
  entry buffer, where `end = entries_total.saturating_sub(offset)` and
  `start = end.saturating_sub(lines)` (two saturating subtractions, no other
  arithmetic)
- when `offset >= entries_total`, the window SHALL be empty
  (`returned_entries_count = 0`); this is a normal terminal page, NOT an error

#### Scenario: Apply default tmux line window

- **WHEN** look request for a tmux target omits `lines`
- **THEN** relay captures using default `lines = 120`

#### Scenario: Apply default ACP entry window

- **WHEN** look request for an ACP target omits `lines`
- **THEN** relay windows using default `lines = 50`

#### Scenario: Reject out-of-range line window

- **WHEN** look request includes `lines` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_lines`

#### Scenario: Window ACP entries backward from newest end

- **WHEN** ACP look request includes `offset` with `offset < entries_total`
- **THEN** relay returns the window ending `offset` entries back from the
  newest entry, sized by `lines`

#### Scenario: Offset beyond available entries yields empty window

- **WHEN** ACP look request includes `offset >= entries_total`
- **THEN** relay returns `snapshot_entries=[]` with `returned_entries_count=0`
- **AND** relay does NOT return an error
- **AND** `entries_total` still reflects the full buffer count

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `structured_entries_v1`)

`bundle_name` is retired from look responses; bundle context is recoverable
from the `target_session` suffix.

When `snapshot_format = "lines"`, responses SHALL include:
- `snapshot_lines` (`string[]`)

When `snapshot_format = "structured_entries_v1"`, responses SHALL include:
- `snapshot_entries` (`object[]`) using canonical ACP entry vocabulary.

For ACP targets, successful relay look responses SHALL additionally include:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `entries_total` (`number`, required) — count of all entries available in
  the buffer before windowing; reflects the full buffer on every response,
  including stale and empty snapshots
- `returned_entries_count` (`number`, required) — count of entries in
  `snapshot_entries` after windowing; SHALL equal the length of
  `snapshot_entries`
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when unavailable)

ACP stale reason vocabulary:

- `acp_worker_initializing`
- `acp_worker_unavailable`
- `acp_snapshot_prime_timeout`
- `acp_stream_stalled`

#### Scenario: Return canonical look payload for tmux target

- **WHEN** look succeeds for tmux target
- **THEN** relay returns `snapshot_format="lines"`
- **AND** includes ordered `snapshot_lines` from oldest to newest
- **AND** ACP additive fields are omitted

#### Scenario: Return ACP look payload with structured entries

- **WHEN** look succeeds for ACP target
- **THEN** relay returns `snapshot_format="structured_entries_v1"`
- **AND** includes `snapshot_entries`
- **AND** includes required ACP additive fields `freshness`,
  `snapshot_source`, `entries_total`, and `returned_entries_count`

#### Scenario: Report entry counts for windowed ACP look

- **WHEN** ACP look returns a window narrower than the full buffer
- **THEN** `entries_total` reflects the full buffer count
- **AND** `returned_entries_count` equals the length of `snapshot_entries`
- **AND** `returned_entries_count <= entries_total`

#### Scenario: Keep required ACP fields when snapshot is empty

- **WHEN** ACP look succeeds with `snapshot_entries=[]`
- **THEN** relay still includes required `freshness`, `snapshot_source`,
  `entries_total`, and `returned_entries_count`
- **AND** `returned_entries_count=0`

### Requirement: Persistent Relay Client Streams

Relay SHALL support long-lived full-duplex Unix socket client streams.

Client request/response frames and relay-pushed event frames SHALL share the
same stream connection.

Relay SHALL reject protocol frames received before successful `hello`
registration with `validation_missing_hello`.

#### Scenario: Accept request/response exchange on persistent stream

- **WHEN** a client opens relay stream and completes `hello`
- **THEN** client can send request frames on that stream
- **AND** relay returns response frames on that same stream without closing it

#### Scenario: Reject request before hello

- **WHEN** client sends request frame before successful `hello`
- **THEN** relay rejects frame with `validation_missing_hello`

### Requirement: Hello Registration Contract

Each client stream SHALL begin with a `hello` registration frame containing:

- `principal_id` (in `<id>@<namespace>` form)
- `identity_token`
- `schema_version`

`hello` SHALL carry identity and credential only. No transport class, mode, or
privilege field is accepted; relay SHALL reject unrecognized fields.

The relay process hosts a single Unix socket at `<state_root>/relay.sock` and
serves all configured bundles through that socket. The namespace portion of
`principal_id` SHALL serve as the connection type indicator:

- Session namespace (`@<bundle_name>`): relay SHALL look the bundle up in the
  bundle catalog and bind the connection to that bundle's runtime for the
  lifetime of the stream. If the bundle is not configured, relay SHALL reject
  with `validation_unknown_bundle`.
- Relay-wide namespaces (`@GLOBAL`, `@EXTERNAL`, `@RELAY`): relay SHALL NOT
  bind the connection to any bundle; the connection is relay-wide.

Credential verification and principal store lookup SHALL proceed as specified
by `add-identity-federation`.

If a second stream attempts `hello` for the same `principal_id` while the
current owner is live, relay SHALL reject the second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for session principal

- **WHEN** a client sends valid `hello` with `principal_id = "master@agentmux"`
- **AND** namespace `agentmux` maps to a configured bundle
- **AND** `master` maps to a configured bundle member
- **THEN** relay accepts hello and binds stream to bundle `agentmux`

#### Scenario: Accept hello for global user principal

- **WHEN** a client sends valid `hello` with `principal_id = "operator@GLOBAL"`
- **AND** credential is valid
- **THEN** relay accepts hello and registers connection relay-wide (no bundle binding)

#### Scenario: Reject hello for unknown bundle

- **WHEN** a client sends `hello` with `principal_id = "session@unknownbundle"`
- **AND** `unknownbundle` is not configured on the running relay
- **THEN** relay rejects with `validation_unknown_bundle`
- **AND** closes the connection without registering a stream

### Requirement: Static Recipient Routability

Static configured recipients from bundle session definitions SHALL remain
routable independent of active stream presence or prior `hello` from those
recipients.

#### Scenario: Route to configured recipient before recipient stream registration

- **WHEN** sender targets a configured bundle recipient
- **AND** recipient has no active stream registration
- **THEN** relay processes routing using configured recipient identity semantics
- **AND** does not reject solely for missing recipient `hello`

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `target_session`
- `created_at`

`target_session` SHALL carry the canonical `session@bundle` form per the
Canonical Session Identity requirement. `bundle_name` is retired; bundle
context is recoverable from the `target_session` suffix.

Event types SHALL include:

- `incoming_message`
- `delivery_outcome`

`incoming_message` payload SHALL include:

- `message_id`
- `sender_session`
- `body`
- optional `cc_sessions`

`delivery_outcome` payload SHALL include:

- `message_id`
- `phase` (`routed`|`delivered`|`failed`)
- `outcome` (`success`|`timeout`|`failed`|null)
- optional `reason_code`
- optional `reason`

`delivery_outcome` SHALL be the canonical machine completion/update carrier for
stream-path delivery updates and SHALL be keyed by `message_id`.

`phase=routed` SHALL be diagnostic metadata and SHALL set `outcome=null`.

Terminal updates SHALL keep existing external vocabulary:

- delivered terminal: `phase=delivered`, `outcome=success`
- failure terminal: `phase=failed`, `outcome` in (`timeout`|`failed`)

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `phase=failed`
- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Push routed diagnostic update

- **WHEN** relay resolves stream routing for a target delivery
- **THEN** relay pushes `delivery_outcome` with `phase=routed`
- **AND** sets `outcome=null`

#### Scenario: Push terminal delivery outcome update

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical `phase` and `outcome` values

#### Scenario: Map dropped_on_shutdown to failed terminal update

- **WHEN** relay terminal state for a target is `dropped_on_shutdown`
- **THEN** `delivery_outcome` includes `phase=failed`
- **AND** includes `outcome=failed`
- **AND** includes `reason_code=dropped_on_shutdown`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`

### Requirement: Stream Failure Semantics

Relay SHALL fail fast on malformed hello/protocol frames.

Relay SHALL surface stream disconnect events through runtime diagnostics and
continue serving other active streams.

#### Scenario: Reject malformed hello payload

- **WHEN** client sends malformed or invalid hello frame
- **THEN** relay rejects with structured validation error
- **AND** does not register stream identity

#### Scenario: Continue serving other streams after one disconnect

- **WHEN** one client stream disconnects unexpectedly
- **THEN** relay records diagnostic event
- **AND** continues serving other active client streams

### Requirement: Policy Preset Source

Relay authorization policy presets SHALL be loaded from:

- `<config-root>/policies.toml`

`policies.toml` SHALL define presets using `[[policies]]` entries with:

- `id` (required)
- `description` (optional)
- `[controls]` (required)

`policies.toml` MAY define:

- `default` (`<policy-id>`)

Relay SHALL fail fast when this artifact is missing or invalid.

#### Scenario: Reject startup when policies file is missing

- **WHEN** relay starts and `<config-root>/policies.toml` is absent
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with implicit fallback policy

#### Scenario: Reject startup when policies file is invalid

- **WHEN** relay starts and `policies.toml` cannot be parsed or validated
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with partial policy state

#### Scenario: Use built-in conservative default when preset default is absent

- **WHEN** `policies.toml` omits top-level `default`
- **AND** a session omits explicit `policy`
- **THEN** relay applies built-in conservative default policy
- **AND** built-in controls are:
  - `find = self`
  - `list = home`
  - `look = self`
  - `send = home`
  - `do` defaults to `none` for unspecified actions

### Requirement: Session Policy Binding

Each session definition SHALL support optional binding to a policy preset id:

- `policy = "<policy-id>"`

If session `policy` is omitted, relay SHALL resolve policy by precedence:

1. top-level `default` preset in `policies.toml` when present
2. built-in conservative default policy

Relay SHALL reject bundle configuration when a session references an unknown
policy id.

#### Scenario: Reject unknown session policy reference

- **WHEN** a session declares `policy = "missing-policy"`
- **AND** `policies.toml` has no matching `[[policies]].id`
- **THEN** relay rejects configuration with a validation error

#### Scenario: Resolve omitted session policy from top-level default

- **WHEN** session omits explicit `policy`
- **AND** `policies.toml` defines top-level `default`
- **THEN** relay uses that default policy preset for the session

### Requirement: Authorization Control Vocabulary

Relay SHALL evaluate authorization using canonical controls and scope values:

- `find`: `none` | `self` | `home` | `all`
- `list`: `none` | `self` | `home` | `all`
- `look`: `none` | `self` | `home` | `all`
- `send`: `none` | `self` | `home` | `all`
- `do`: map `action_id -> (none | self | home | all)`

The policies file is authoritative: every control accepts the full scope
ladder at parse time, and the consuming authorization checks give each value
its effect via scope rank order.

For current self-target-only `do` MVP behavior:

- `none` and `self` are operative
- `home` and `all` are reserved/non-operative until non-self `do`
  targeting is introduced

#### Scenario: Evaluate look request using configured look scope

- **WHEN** relay evaluates a `look` request
- **THEN** it uses the session policy control `look`
- **AND** applies one of the canonical scope values

#### Scenario: Treat missing do action entry as none

- **WHEN** relay evaluates `do` authorization
- **AND** requested action id is not present in `do` control map
- **THEN** relay treats authorization scope as `none`

#### Scenario: Treat do all-home/all-all scopes as reserved in current MVP

- **WHEN** relay evaluates `do` authorization
- **AND** action scope is `home` or `all`
- **THEN** relay treats scope as reserved/non-operative for current MVP
- **AND** non-self `do` execution remains unsupported by runtime contract

### Requirement: Centralized Authorization Decision Point

Relay SHALL be the centralized authorization decision point.
CLI and MCP SHALL remain validators/adapters and SHALL NOT implement
independent authorization decision logic.

#### Scenario: Return relay-authored denial across surfaces

- **WHEN** a request is denied by policy
- **THEN** relay returns canonical denial response
- **AND** CLI/MCP propagate that denial without re-evaluating authorization

### Requirement: Authorization Evaluation Order

Relay SHALL evaluate requests in this order:

1. request validation
2. requester identity resolution
3. bundle/target/action resolution
4. authorization policy evaluation
5. execution

Validation failures SHALL take precedence over authorization denials.

#### Scenario: Prefer validation failure over authorization denial for non-send target

- **WHEN** a non-send request includes an unknown target session
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

#### Scenario: Prefer send explicit-target validation over authorization denial

- **WHEN** a send request includes an unknown or non-canonical explicit target
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

### Requirement: Authorization Denial Schema

When relay denies a valid/resolved request by policy, relay SHALL return
`authorization_forbidden` with `details` including:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

#### Scenario: Return canonical denial details for single-target operation

- **WHEN** relay denies a same-bundle non-self look request by policy
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include required fields
- **AND** denial details include `target_session`

### Requirement: Relay List Authorization

Relay `list_sessions` responses SHALL require policy evaluation for capability
`list.read`.
If requester identity is valid and list access is denied by policy, relay SHALL
return `authorization_forbidden` and SHALL NOT return successful list payload.

The successful list payload collection key SHALL be `principals[]` on the
canonical `ListedBundle` payload (renamed from `sessions[]`); the per-entry
`ListedSession` shape is unchanged.

#### Scenario: Deny list_sessions without successful payload

- **WHEN** requester identity is valid
- **AND** policy denies `list.read` for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return a successful `bundle.principals[]` payload

### Requirement: Relay Send Scope Control

Relay send authorization SHALL be driven by `send` control scope, evaluated
against the requester's dispatch (home) bundle policy:

- `home` allows only same-bundle targets
- `all` allows cross-bundle targets

Cross-bundle send SHALL require `all`; a cross-bundle send issued under
`home` SHALL be rejected with `authorization_forbidden`.

#### Scenario: Reject cross-bundle send under home-only scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "home"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Permit cross-bundle send under all-all scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all"`
- **THEN** relay routes and delivers to the cross-bundle target(s)

### Requirement: Authorization Hooks for Do and Find

Relay SHALL reserve authorization hooks for:

- `do` action-id scoped controls
- `find` scope controls

These hooks SHALL use the same evaluation order and denial schema as `list`,
`send`, and `look`.

#### Scenario: Deny do action run with canonical schema

- **WHEN** relay denies action execution by `do` control map
- **THEN** relay returns `authorization_forbidden`
- **AND** details include canonical required fields

#### Scenario: Deny do action run when do map sets none

- **WHEN** requested action id maps to `none` in `do` control map
- **THEN** relay returns `authorization_forbidden`

### Requirement: Relay Bundle Lifecycle Operations

Relay SHALL support explicit bundle lifecycle transition operations:

- `up` (host selected bundle runtimes)
- `down` (unhost selected bundle runtimes)

These operations SHALL control bundle hosting state and SHALL NOT terminate the
relay process itself.

`up/down` SHALL be idempotent:

- `up` on an already hosted bundle returns `outcome=skipped` with
  `reason_code=already_hosted`
- `down` on an already unhosted bundle returns `outcome=skipped` with
  `reason_code=already_unhosted`

`up/down` result payloads SHALL preserve selector-resolved bundle order.

Bundle startup outcomes SHALL be scoped to bundle lifecycle evaluation and SHALL
NOT relock process-level no-selector `agentmux host relay` startup success
semantics.

#### Scenario: Keep relay process alive after down transition

- **WHEN** relay processes `down` for one or more bundles
- **THEN** relay updates bundle hosting state
- **AND** relay process remains running

#### Scenario: Report idempotent up transition

- **WHEN** relay processes `up` for a bundle already hosted by current runtime
- **THEN** result entry uses `outcome=skipped`
- **AND** sets `reason_code=already_hosted`

#### Scenario: Report idempotent down transition

- **WHEN** relay processes `down` for a bundle not currently hosted
- **THEN** result entry uses `outcome=skipped`
- **AND** sets `reason_code=already_unhosted`

### Requirement: Relay Bundle Lifecycle Result Contract

Relay bundle lifecycle responses for `up/down` SHALL include:

- `schema_version`
- `action` (`up`|`down`)
- `bundles` array entries with:
  - `bundle_name`
  - `outcome` (`hosted`|`unhosted`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
- aggregate fields:
  - `changed_bundle_count`
  - `skipped_bundle_count`
  - `failed_bundle_count`
  - `changed_any`

For `up`, lock contention MAY produce:

- `outcome=skipped`
- `reason_code=lock_held`

#### Scenario: Emit canonical up lifecycle payload

- **WHEN** relay completes an `up` operation
- **THEN** response matches canonical lifecycle result contract

#### Scenario: Emit canonical down lifecycle payload

- **WHEN** relay completes a `down` operation
- **THEN** response matches canonical lifecycle result contract

### Requirement: Bundle Configuration Includes Autostart Eligibility

Per-bundle TOML configuration SHALL support optional top-level `autostart`
boolean with default `false`.

`autostart` SHALL indicate eligibility for no-selector relay host autostart mode
and SHALL NOT change bundle routing identity semantics.

#### Scenario: Accept bundle file with autostart true

- **WHEN** bundle file includes `autostart = true`
- **THEN** configuration loads successfully

#### Scenario: Accept bundle file without autostart field

- **WHEN** bundle file omits `autostart`
- **THEN** configuration loads successfully
- **AND** runtime treats bundle as not autostart-eligible

### Requirement: ACP Look Snapshot Contract

Relay look SHALL support ACP-backed target sessions using an in-memory
snapshot held by the shared per-target ACP worker/client. Snapshot data
SHALL NOT be persisted to disk; the upstream ACP server is the
authoritative source of conversation history and provides full replay
via `session/load` on worker reconnect.

For ACP targets, relay SHALL:
- use the same shared per-target ACP worker/client used by ACP send
  lifecycle and prompt execution,
- ingest replay content from `session/load` as baseline snapshot
  replacement (in-memory),
- ingest replay content from live `session/update` as append in ACP
  receive order (in-memory),
- preserve source order (oldest -> newest) without dedupe in MVP,
- retain at most 1000 ACP snapshot entries per session in the in-memory
  buffer,
- evict oldest entries first when retention exceeds 1000,
- return look results ordered oldest -> newest,
- avoid spawning a second ACP client for steady-state look requests,
- read look snapshot via a non-draining accessor exposed by the ACP
  client (the existing draining `take_replay_entries` accessor is
  reserved for non-look consumers such as the debug TUI binary).

Canonical ACP snapshot entry vocabulary SHALL be:
- `kind = "user"` with `lines: string[]`
- `kind = "agent"` with `lines: string[]`
- `kind = "cognition"` with `lines: string[]`
- `kind = "invocation"` with `call_id: string` (upstream-issued
  correlation token), `status: "pending"|"completed"`,
  `invocation: object` (pass-through tool-call structure), and optional
  `result: object` (pass-through tool-result structure when status is
  completed). Coalesced form: a single entry carries both the call and
  its result; no separate result entry.
- `kind = "update"` with `update_kind: string`, `lines: string[]` for
  fallback unknown/unsupported updates (MUST NOT be dropped).

Relay SHALL NOT inject ANSI/control sequences into ACP snapshot
entries.

Relay restart behavior SHALL be:
- on relay startup, the worker reconnects to the upstream session via
  `session/load` using the persisted `acp_session_id` (see ACP Session
  Identity Persistence Ownership),
- if no usable persisted session id exists, the worker creates a new
  upstream session via `session/new`,
- the in-memory snapshot is rebuilt from the upstream replay; first
  `look` during prime returns fresh-but-empty with the appropriate
  `stale_reason_code` (see ACP Look Freshness Derivation).

#### Scenario: Use shared ACP worker as authoritative look snapshot writer

- **WHEN** relay serves ACP send and ACP look for one target session
- **THEN** both operations use one shared per-target ACP worker/client
- **AND** relay does not create a separate look-only ACP client for that target

#### Scenario: Replace baseline from load then append live updates in order

- **WHEN** relay receives `session/load` replay for target session
- **AND** later observes live `session/update` replay entries
- **THEN** relay replaces the in-memory snapshot baseline from load replay
- **AND** appends live snapshot entries in ACP receive order
- **AND** preserves oldest->newest ordering in look responses

#### Scenario: Preserve unknown replay kinds via fallback entry

- **WHEN** relay observes an unknown or unsupported replay/update kind
- **THEN** relay emits fallback entry `kind="update"`
- **AND** relay does not silently drop the observed update

#### Scenario: Coalesce tool call and result onto one invocation entry

- **WHEN** relay observes a `tool_call` replay entry followed by a matching `tool_call_update`
- **THEN** the in-memory snapshot carries one entry with `kind="invocation"`, `status="completed"`, the original `invocation` payload, and the `result` payload inline
- **AND** no separate `kind="result"` entry is emitted

#### Scenario: Look returns fresh-but-empty during cold-start prime

- **WHEN** relay restarts and serves `look` before the worker has completed `session/load`
- **THEN** the response carries `freshness=stale` with `stale_reason_code=acp_worker_initializing`
- **AND** subsequent `look` after prime completes returns the full upstream-replayed transcript

#### Scenario: Evict oldest in-memory snapshot entries beyond retention cap

- **WHEN** the in-memory ACP snapshot reaches 1000 entries for a target session
- **AND** an additional entry is ingested
- **THEN** the oldest entry is evicted
- **AND** look responses continue to return up to 1000 most recent entries

### Requirement: ACP Send Lifecycle Selection Precedence

For ACP-backed send operations, runtime lifecycle selection SHALL use this
precedence order:

1. session config `coder-session-id` when present
2. relay-managed persisted ACP session id for that bundle session when present
3. otherwise `session/new`

This precedence supersedes coder-session-id-only lifecycle selection for ACP
send operations.

#### Scenario: Prefer configured coder-session-id for load

- **WHEN** target session is ACP-backed
- **AND** session config includes `coder-session-id`
- **THEN** relay selects ACP `session/load` using that configured id

#### Scenario: Use persisted session id when config id is absent

- **WHEN** target session is ACP-backed
- **AND** session config omits `coder-session-id`
- **AND** relay has a persisted ACP session id for that bundle session
- **THEN** relay selects ACP `session/load` using the persisted id

#### Scenario: Select session-new when no load identity exists

- **WHEN** target session is ACP-backed
- **AND** session config omits `coder-session-id`
- **AND** relay has no persisted ACP session id for that bundle session
- **THEN** relay selects ACP `session/new`

### Requirement: ACP Session Identity Persistence Ownership

Relay SHALL maintain durable ACP session-id state for ACP-backed bundle
sessions under runtime state ownership.

Relay SHALL update persisted ACP session-id state when ACP `session/new`
returns a new `sessionId`.

#### Scenario: Persist session id returned by session-new

- **WHEN** relay executes ACP `session/new` for an ACP-backed session
- **AND** ACP response includes `sessionId`
- **THEN** relay persists that `sessionId` for subsequent lifecycle selection

#### Scenario: Keep persisted state scoped to bundle session identity

- **WHEN** relay persists ACP session id state
- **THEN** the persisted value is associated with one bundle session identity
- **AND** is not reused across unrelated bundle sessions

### Requirement: ACP Load Path Fail-Fast Semantics

When ACP `session/load` is selected, load failure SHALL fail the target send
operation and SHALL NOT fall back to ACP `session/new` in the same operation.

#### Scenario: Fail send target on session-load failure

- **WHEN** relay selects ACP `session/load`
- **AND** the load operation fails
- **THEN** relay reports target send outcome as failed
- **AND** relay does not call ACP `session/new` for that target in the same
  send operation

### Requirement: ACP Capability Gating

Relay SHALL perform explicit ACP capability gating before lifecycle/prompt
execution.

Required gates:

- ACP `initialize` must succeed
- ACP `session/load` path requires advertised load-session capability
- ACP prompt path requires prompt-session capability

Capability-gating failures SHALL use canonical error taxonomy:

- ACP initialize failure SHALL return `runtime_acp_initialize_failed`
- missing ACP capability for load/prompt SHALL return
  `validation_missing_acp_capability`

For `validation_missing_acp_capability`, error details SHALL include:

- `target_session`
- `required_capability` (`session/load` | `session/prompt`)
- `reason`

#### Scenario: Reject load path when load capability is missing

- **WHEN** relay selects ACP `session/load`
- **AND** initialized ACP capabilities do not advertise load-session support
- **THEN** relay fails the target with `validation_missing_acp_capability`
- **AND** error details include
  `required_capability = "session/load"`

#### Scenario: Reject prompt path when prompt capability is missing

- **WHEN** relay attempts ACP prompt execution for target
- **AND** initialized ACP capabilities do not advertise prompt-session support
- **THEN** relay fails the target with `validation_missing_acp_capability`
- **AND** error details include
  `required_capability = "session/prompt"`

#### Scenario: Surface initialize failure with canonical runtime code

- **WHEN** relay cannot complete ACP initialize handshake
- **THEN** relay fails target processing with `runtime_acp_initialize_failed`

### Requirement: ACP Stop-Reason Outcome Mapping

Relay SHALL map ACP prompt terminal states into canonical send outcomes with
stable reason-code behavior.

Mapping SHALL include:

- ACP terminal stop reasons (`end_turn`, `max_tokens`, `max_turn_requests`,
  `refusal`) -> delivery outcome `delivered` with `reason_code = null`
- ACP terminal stop reason `cancelled` -> delivery outcome `failed` with
  `reason_code = acp_stop_cancelled`
- ACP dropped-on-shutdown behavior -> delivery outcome `failed` with
  `reason_code = dropped_on_shutdown`
- ACP turn timeout -> delivery outcome `timeout` with
  `reason_code = acp_turn_timeout`

#### Scenario: Map successful ACP terminal stop reasons to delivered

- **WHEN** ACP prompt turn completes with terminal stop reason `end_turn`
- **THEN** relay reports target delivery outcome `delivered`
- **AND** sets `reason_code = null`

#### Scenario: Map cancelled to failed outcome

- **WHEN** ACP prompt turn completes with stop reason `cancelled`
- **THEN** relay reports target delivery outcome `failed`
- **AND** sets `reason_code = acp_stop_cancelled`

#### Scenario: Map ACP turn timeout to timeout outcome

- **WHEN** ACP prompt turn does not complete before effective turn-wait timeout
- **THEN** relay reports target delivery outcome `timeout`
- **AND** sets `reason_code = acp_turn_timeout`

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals from the background reader to
maintain internal worker readiness state for scheduling.

MVP state model:

- `available`: worker healthy and ready for next prompt
- `busy`: prompt accepted and turn in progress
- `unavailable`: worker transport/process failure requiring restart

Transition contract:

- successful prompt write to ACP stdin => `busy`
- background reader observes terminal `stopReason` in prompt response => `available`
- stdin write failure OR reader thread exit (any cause) => `unavailable`

The `busy` transition now occurs on write-success, not on first `session/update`
observation. Reader thread exit is an additional `unavailable` trigger,
mirroring the write-failure path.

MVP sender-surface contract:

- these transitions SHALL NOT require additional sender-facing `send` outputs
- send success semantics remain phase-1 delivery acknowledgment only

#### Scenario: Mark worker busy on prompt write success

- **WHEN** relay successfully writes a prompt to ACP stdin
- **THEN** relay marks worker state as `busy`

#### Scenario: Mark worker available on terminal stopReason

- **WHEN** ACP background reader receives JSON-RPC response to the prompt
  request-id with a terminal `stopReason`
- **THEN** relay marks worker state as `available`
- **AND** subsequent sends MAY be admitted for that target

#### Scenario: Mark worker unavailable on reader thread exit

- **WHEN** the ACP background reader thread exits (EOF, I/O error, or panic)
- **THEN** relay marks worker state as `unavailable`
- **AND** pending requests are drained with an error

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends and ACP look
snapshot ingestion.

Worker model SHALL be:

- one worker per target session (one child process, one background reader thread)
- serialized request queue per worker
- fixed MVP queue bound `max_pending = 64`
- initialized during bundle startup/session startup pass for hosted bundles
- anchored by relay runtime context (relay socket/runtime directory), not tmux
  transport semantics
- never lazily created by ACP send/look request handlers

Worker startup sequence SHALL be:

1. spawn ACP child process
2. start background reader thread (owns child stdout)
3. initialize (register request-id, write to stdin, wait on oneshot)
4. select lifecycle (`session/load` when identity exists, else `session/new`)
5. worker transitions to `available` and accepts prompts

Worker shutdown sequence SHALL be:

1. close shared `Arc<Mutex<ChildStdin>>` (signal EOF to child)
2. drop child process handle
3. `join` background reader thread
4. release per-session state (replay buffer, pending-request registry)

Backpressure contract:

- enqueue beyond bound SHALL fail with `runtime_acp_queue_full`

Disconnect/restart contract:

- stdin write failure before phase-1 acknowledgment =>
  `runtime_acp_connection_closed`
- reader thread exit after phase-1 acknowledgment SHALL keep response
  immutable and transition worker to `unavailable` for recovery

Failure taxonomy SHALL include:

- `runtime_acp_initialize_failed`
- `runtime_acp_session_load_failed`
- `runtime_acp_session_new_failed`
- `runtime_acp_prompt_failed`
- `acp_turn_timeout`
- `runtime_acp_worker_unavailable`
- `transport_unavailable` (ACP child write failure or reader thread exit)

#### Scenario: Keep one authoritative worker for ACP send and look ingestion

- **WHEN** relay handles ACP send requests and ACP look reads for one target
- **THEN** lifecycle/reconnect ownership remains with one shared worker
- **AND** relay avoids dual ACP worker/client ownership for that target

#### Scenario: Start ACP worker during startup pass without lazy send/look bootstrap

- **WHEN** relay runs startup pass for a hosted bundle with ACP targets
- **THEN** relay initializes one ACP worker per configured ACP target
- **AND** ACP send/look request handlers do not lazily create ACP workers

#### Scenario: Return deterministic unavailable outcome when ACP worker is absent

- **WHEN** ACP send or ACP look is requested for a target whose ACP worker is
  unavailable
- **THEN** relay does not spawn a request-scoped ACP client
- **AND** send returns failure with `runtime_acp_worker_unavailable`
- **AND** look returns stale metadata with
  `stale_reason_code=acp_worker_unavailable`

#### Scenario: Worker teardown joins reader thread before releasing state

- **WHEN** an ACP worker is torn down (idle timeout, target removed, bundle stop)
- **THEN** relay closes child stdin, drops child process handle, and joins the
  reader thread before releasing per-session state
- **AND** no per-session state is accessed after join completes

### Requirement: ACP Permission Request Readiness Signal

Relay SHALL treat ACP `session/request_permission` as in-progress turn activity
for ACP readiness tracking in MVP.

MVP behavior contract:

- `session/request_permission` observed before terminal completion SHALL count
  as first activity for two-phase sync acknowledgment semantics
- worker readiness SHALL transition to `busy` while turn completion remains
  pending
- terminal stopReason completion SHALL transition readiness to `available`

#### Scenario: Treat permission request as first ACP activity

- **WHEN** relay observes ACP `session/request_permission` before prompt result
- **THEN** relay marks worker state `busy` for the duration of the permission turn
- **AND** worker returns to `available` only after terminal stopReason is observed

#### Scenario: Keep worker non-ready while permission turn is in progress

- **WHEN** ACP `session/request_permission` is observed mid-turn
- **THEN** relay marks worker state `busy`
- **AND** relay does not consider that worker ready for next delivery until
  terminal stopReason is observed

### Requirement: UI Request-Path Sender Validation

Relay SHALL validate non-hello request-path UI sender identities using global
TUI sessions from `<config-root>/tui.toml`.

For request-path operations such as `send`, relay SHALL:

1. validate sender `session_id` exists in global TUI sessions,
2. evaluate authorization using that TUI session's `policy` reference,
3. return canonical `authorization_forbidden` when policy denies.

#### Scenario: Authorize send using global UI session policy

- **WHEN** relay receives `send` request with UI sender `session_id = "user"`
- **AND** global TUI sessions include `id = "user"` with `policy = "ui-default"`
- **THEN** relay evaluates authorization using policy `ui-default`

#### Scenario: Reject request-path sender missing from global UI sessions

- **WHEN** relay receives `send` request with UI sender `session_id = "ghost"`
- **AND** no global TUI session maps to `id = "ghost"`
- **THEN** relay rejects request with `validation_unknown_sender`

### Requirement: Relay List Sessions Request Scope

Relay SHALL support only single-bundle session listing requests in MVP.
Relay SHALL NOT accept all-bundle list selectors.

#### Scenario: Reject all-bundle relay list selector

- **WHEN** a caller requests relay list with all-bundle selector semantics
- **THEN** relay rejects request with `validation_invalid_params`

### Requirement: Bundle Startup Evaluation Boundary

Relay bundle startup SHALL evaluate outcomes in two deterministic phases:

1. bundle preflight phase,
2. per-session startup pass phase.

When preflight succeeds, relay SHALL attempt startup for all configured
sessions in that bundle during one startup pass.
Startup outcome SHALL be computed after that startup pass completes.

When preflight fails, relay SHALL:

- mark bundle state as `down`,
- set `state_reason_code=runtime_startup_failed`,
- skip the per-session startup pass.

Per-transport readiness predicates in MVP:

- tmux session is ready when configured session exists and relay resolves an
  active pane target.
- ACP session is ready when shared per-target ACP worker reaches ready state and
  lifecycle selection succeeds (`session/load` or `session/new` per existing
  contract).

#### Scenario: Attempt all configured sessions after successful preflight

- **WHEN** preflight succeeds for a bundle startup request
- **THEN** relay attempts startup for all configured sessions in that bundle
- **AND** relay evaluates startup outcome only after the pass completes

#### Scenario: Fail preflight before per-session startup pass

- **WHEN** bundle preflight fails
- **THEN** relay marks bundle `state=down`
- **AND** sets `state_reason_code=runtime_startup_failed`
- **AND** does not run the per-session startup pass

### Requirement: Bundle Startup Health Model

Relay list payloads SHALL preserve bundle `state` as `up|down`.
For `state=up`, relay SHALL include required additive field
`startup_health` with value `healthy|degraded`.

Startup health semantics:

- `state=up`, `startup_health=healthy` when all configured sessions are ready.
- `state=up`, `startup_health=degraded` when at least one configured session is
  ready and at least one startup attempt failed.
- `state=down` when zero configured sessions are ready.

For empty bundles (`members=[]`), relay SHALL return:

- `state=down`
- `state_reason_code=runtime_no_configured_sessions`

#### Scenario: Return degraded startup health with partial session success

- **WHEN** at least one configured session becomes ready
- **AND** at least one configured session startup attempt fails
- **THEN** relay reports `state=up`
- **AND** includes `startup_health=degraded`

#### Scenario: Return down state for zero ready sessions

- **WHEN** zero configured sessions are ready after startup evaluation
- **THEN** relay reports `state=down`

#### Scenario: Return empty-bundle down reason

- **WHEN** bundle configuration contains zero sessions
- **THEN** relay reports `state=down`
- **AND** sets `state_reason_code=runtime_no_configured_sessions`

### Requirement: Startup Failure Visibility Contract

Relay SHALL provide machine-readable startup failure visibility via:

1. live per-session startup failure event/inscription:
   `relay.session_start_failed`,
2. persisted bounded per-bundle startup failure history.

Persisted history contract in MVP:

- fixed bound `max_startup_failures=256`,
- oldest-first eviction when bound is exceeded,
- response ordering oldest -> newest,
- monotonic per-bundle `sequence` field per failure record,
- history persists across relay restarts,
- history clears when bundle runtime state is explicitly reset/removed.

Each startup-failure record SHALL include:

- `bundle_name`
- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

Relay list payloads SHALL include:

- `startup_failure_count` (required integer),
- `recent_startup_failures` (required bounded array; may be empty).

#### Scenario: Emit canonical startup-failure event

- **WHEN** one session startup attempt fails during startup pass
- **THEN** relay emits `relay.session_start_failed`
- **AND** event payload includes canonical startup-failure fields

#### Scenario: Expose bounded startup-failure history in list payload

- **WHEN** startup-failure history exists for a bundle
- **THEN** relay list payload includes `startup_failure_count`
- **AND** includes `recent_startup_failures` ordered oldest -> newest

#### Scenario: Evict oldest startup-failure history record at bound

- **WHEN** a new startup-failure record is persisted and bundle history already
  contains 256 records
- **THEN** relay evicts the oldest record first

### Requirement: Bundle Down Reason Precedence

When relay reports `state=down`, `state_reason_code` precedence SHALL be:

1. `runtime_no_configured_sessions` (empty bundle),
2. `runtime_startup_failed` (preflight failure or all configured sessions
   failed startup pass).

Relay SHALL preserve process-level host startup summary semantics for
`runtime_listener_bind_failed`; this code is not part of bundle list-state
reason precedence.

#### Scenario: Prefer no-configured-sessions reason over startup-failed reason

- **WHEN** bundle has zero configured sessions
- **THEN** relay reports `state_reason_code=runtime_no_configured_sessions`

#### Scenario: Use startup-failed reason when startup pass yields zero ready sessions

- **WHEN** bundle preflight succeeds
- **AND** startup pass completes with zero ready sessions
- **THEN** relay reports `state_reason_code=runtime_startup_failed`

### Requirement: ACP Look Freshness Derivation

Relay SHALL evaluate ACP look freshness deterministically from shared worker
state and update recency.

MVP deterministic thresholds:

- `acp_look_prime_timeout_ms = 750`
- `acp_stream_stalled_after_ms = 5000`

Authoritative age source precedence SHALL be:
1. `last_acp_frame_observed_at_ms` (updated by background reader on each
   `session/update` notification)
2. `last_snapshot_update_ms`
3. age unavailable (omit `snapshot_age_ms`)

Freshness predicate order SHALL be:
1. `worker_state=unavailable` => `freshness=stale`,
   `stale_reason_code=acp_worker_unavailable`.
2. snapshot empty:
   - prime timeout => `stale_reason_code=acp_snapshot_prime_timeout`
   - otherwise => `stale_reason_code=acp_worker_initializing`
3. snapshot non-empty:
   - `worker_state=busy` => `freshness=fresh` (MUST NOT emit
     `acp_stream_stalled`)
   - `worker_state=initializing` => `freshness=stale`,
     `stale_reason_code=acp_worker_initializing`
   - `worker_state=available` => `freshness=stale` with
     `stale_reason_code=acp_stream_stalled` only when stalled threshold is
     exceeded using authoritative age source precedence.

Freshness vocabulary semantics under continuous-reader model:

- `LiveBuffer` means "background reader thread is alive and feeding updates"
  (previously: "served from in-memory after request-scoped drain")
- `acp_stream_stalled` means "reader alive but observed N seconds of silence
  on the ACP stream" (previously: "drain window saw nothing")

Wire tokens (`Fresh`/`Stale`, `LiveBuffer`/`None`) are unchanged.

Relay SHALL treat machine freshness status as response-visible state for ACP
look.

Inscriptions/events MAY include the same freshness data as additive telemetry
but SHALL NOT be the sole machine carrier.

#### Scenario: Mark ACP look stale when worker is unavailable

- **WHEN** ACP look is served and shared worker state is unavailable
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_worker_unavailable`

#### Scenario: Keep ACP look fresh while worker is busy and snapshot exists

- **WHEN** ACP look is served with non-empty snapshot entries
- **AND** shared worker state is busy
- **THEN** relay returns `freshness=fresh`
- **AND** relay does not emit `stale_reason_code=acp_stream_stalled`

#### Scenario: Mark ACP look stale when available worker stream is stalled

- **WHEN** shared ACP worker is available
- **AND** no new ACP updates are observed for at least 5000ms using
  authoritative age-source precedence
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_stream_stalled`

#### Scenario: Background reader updates last_acp_frame_observed_at on each update

- **WHEN** background reader receives a `session/update` notification
- **THEN** `last_acp_frame_observed_at_ms` is updated to the current time
- **AND** freshness stalled threshold uses this timestamp as authoritative age

### Requirement: Relay raww operation contract

Relay SHALL expose a raw direct-write operation named `raww` for a single
explicit target session.

Request contract (MVP):
- `target_session` (required)
- `text` (required UTF-8 string)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)
- optional bundle selector with same-bundle-only enforcement

`raww` SHALL NOT support broadcast in MVP.

#### Scenario: Reject raww broadcast shape

- **WHEN** caller attempts to invoke `raww` without one explicit
  `target_session`
- **THEN** relay rejects the request with `validation_invalid_params`

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

### Requirement: Relay raww authorization mapping

Relay SHALL evaluate raww authorization using policy control `raww`.

Policy scope contract:
- allowed values: `none`, `self`, `home`, `all`
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

#### Scenario: Cross-bundle raww permitted under all

- **WHEN** requester policy sets `raww = "all"`
- **AND** requester invokes raww to a session in a different bundle
- **THEN** relay routes to the target and delivers

### Requirement: Relay raww transport behavior

Relay raww transport execution SHALL map as follows:
- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using existing shared ACP worker/client path via
  `session/prompt`

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

### Requirement: Relay raww response contract

Relay raww immediate success responses SHALL be queued-only: the response
confirms enqueue acceptance and SHALL NOT include terminal delivery outcome.
Terminal outcomes are reported out-of-band via `delivery_outcome` stream events.

Required success fields:
- `status` (value `queued`)
- `target_session`
- `transport`

Optional success fields:
- `request_id`
- `message_id`

Failure responses SHALL use canonical relay error payload shape (`code`,
`message`, optional `details`). Only enqueue-time failures (e.g. ACP worker
unavailable) surface synchronously.

#### Scenario: Return queued payload for raww dispatch

- **WHEN** raww request to any writable target is accepted at dispatch boundary
- **THEN** relay returns success with `status = "queued"`
- **AND** includes required fields `target_session` and `transport`

### Requirement: Relay raww input bounds

Relay raww SHALL accept UTF-8 multiline text in MVP and SHALL reject payloads
larger than 32 KiB (UTF-8 bytes) with `validation_invalid_params`.

#### Scenario: Reject oversized raww text payload

- **WHEN** raww `text` exceeds 32 KiB UTF-8 bytes
- **THEN** relay rejects with `validation_invalid_params`

### Requirement: ACP Transport Error Code

Relay SHALL use dedicated error code `transport_unavailable` (in ACP context:
`acp_child_unavailable`) for failures caused by ACP child process write
failures or reader thread exit. This code SHALL be distinguishable from
`internal_unexpected_failure` which covers relay-internal logic errors.

Error code taxonomy addendum:

- `transport_unavailable` — ACP stdin write failure (child process dead or
  pipe broken); caller can retry by requesting a worker reconnect
- `internal_unexpected_failure` — relay-internal logic or lock failure;
  not a transport concern

#### Scenario: ACP stdin write failure returns transport_unavailable

- **WHEN** relay attempts to write a prompt to ACP stdin
- **AND** the write fails with an I/O error
- **THEN** relay returns error code `transport_unavailable`
- **AND** does not return `internal_unexpected_failure`

#### Scenario: transport_unavailable is distinguishable by MCP consumers

- **WHEN** MCP consumer receives an error response for an ACP send
- **AND** error code is `transport_unavailable`
- **THEN** consumer can infer the ACP process is gone and may retry/reattach
- **AND** can distinguish this from a non-retryable relay-internal failure

### Requirement: Choice Decision Capability Contract

Relay SHALL evaluate ACP choice-request decision authority using policy
capability `choose`.

- allowed values: `none`, `self`, `home`, `all`
- default when omitted: `none`
- unknown values SHALL fail validation with `validation_invalid_policy_scope`

#### Scenario: Reject unknown choose scope value

- **WHEN** policy configuration sets `choose` to a value outside the canonical
  scope ladder
- **THEN** relay rejects configuration with `validation_invalid_policy_scope`

#### Scenario: Default omitted choose to none

- **WHEN** policy omits `choose`
- **THEN** relay treats `choose` as `none`

### Requirement: Non-Spoofable Decision Actor Identity

Relay SHALL derive choice decision actor identity from the authenticated
stream context and SHALL NOT trust caller-supplied identity fields in the
action payload.

#### Scenario: Derive decision actor from authenticated stream context

- **WHEN** relay processes a `choices.pick` request
- **THEN** relay derives `decided_by` from the authenticated stream context
- **AND** does not consult any caller-supplied identity field in the payload

### Requirement: Same-Bundle Choice Decision Scope

Choice request routing and decisioning SHALL be same-bundle only in alpha.
The bundle scope SHALL be derived from the request's routing namespace
(frame-level namespace, defaulting to the connection's bound bundle); no
caller-supplied in-payload bundle selector is accepted in `ChoicesPick`.
A caller reaches a bundle's choice queue only by routing to that bundle,
subject to its policy controls.

#### Scenario: Permission decision scoped to session's associated bundle

- **WHEN** a choose-authorized principal issues `ChoicesPick`
- **THEN** relay resolves the choice request within the principal's
  associated bundle
- **AND** no `bundle_name` field is accepted in the request payload

### Requirement: Bounded Choice Queue and Replay

Relay SHALL queue ACP choice requests when no choose-authorized UI is
connected.

Queue contract:

- bundle-scoped global FIFO ordering by monotonic enqueue `sequence`
- `pending_max` default `256`
- optional `[relay.choices] pending-max` override in `1..4096`
- enqueue beyond bound SHALL fail with `runtime_choices_queue_full`

Connect/reconnect replay contract:

- relay emits `choices.snapshot` first
- relay then replays pending `choices.requested` oldest→newest
- replay is at-least-once; consumers dedupe by `choice_request_id`

Naming note: the TOML key remains `pending-max` (decoded to `pending_max` in
the deserialized struct). Relay code stores the queue bound as
`choices_pending_max` on `AuthorizationContext`. The ACP chooser closure
captures this per-bundle constant at worker construction and passes it directly
to the queue, so it does not ride `DeliveryContext`, `ChoiceToMake`, or the
per-delivery task; only the genuinely per-delivery decider list does.

#### Scenario: Reject enqueue beyond queue bound

- **WHEN** pending queue depth equals effective `pending_max`
- **AND** another choice request is queued
- **THEN** relay fails with `runtime_choices_queue_full`

#### Scenario: Emit snapshot then replay on authorized ui connect

- **WHEN** choose-authorized UI connects
- **THEN** relay emits `choices.snapshot` before replay
- **AND** replays pending requests in FIFO order

### Requirement: Durable Pending Queue Restoration

Relay SHALL persist pending choice queue state across restart.
If persisted state is unreadable or corrupt, relay SHALL fail fast with
`runtime_choices_queue_unavailable` for that bundle and SHALL NOT silently
drop pending items.

#### Scenario: Fail fast on unrecoverable queue state

- **WHEN** relay startup cannot restore pending choice queue state
- **THEN** relay fails with `runtime_choices_queue_unavailable`

### Requirement: Non-Expiring Choice Pending Lifecycle

Alpha choice requests SHALL be non-expiring while relay and worker state
remain healthy.

Pending requests SHALL remain pending until one of:

- explicit authorized `selected` decision
- explicit authorized `cancelled` decision
- hard terminal cancellation condition (for example session/worker termination
  or aborted choice wait)

Relay SHALL NOT apply timer-based auto-expiry for choice requests in alpha.
ACP send turn-timeout fields (`acp_turn_timeout_ms`, `[coders.acp] turn-timeout-ms`)
remain independent from choice decision lifecycle.

#### Scenario: Keep choice request pending without timer expiry

- **WHEN** choice request is queued and no decision is made
- **AND** relay/worker remain healthy
- **THEN** request remains pending and is not auto-expired by timer

### Requirement: Choice Lifecycle Event Carrier

Relay stream events SHALL be canonical machine carrier for choice lifecycle.

Required event names:

- `choices.snapshot`
- `choices.requested`
- `choices.resolved`

Required correlation keys on lifecycle events:

- `message_id`
- `choice_request_id`

Required minimum event fields:

- `choices.snapshot`: `message_id`, `choice_request_ids` (array of pending
  request ids; used by consumers to deduplicate the subsequent replay)
- `choices.requested`: `message_id`, `choice_request_id`,
  `target_session`, `requested_kind`, `requested_details`, `enqueued_at`
- `choices.resolved`: `message_id`, `choice_request_id`, `outcome`,
  `reason_code`, `decided_by`, `resolved_at`

Inscriptions MAY be emitted but SHALL be additive only.

#### Scenario: Emit canonical resolved event with correlation keys

- **WHEN** choice request reaches terminal resolution
- **THEN** relay emits `choices.resolved`
- **AND** event includes `message_id` and `choice_request_id`

### Requirement: Choice Resolution and Enforcement Mapping

Relay SHALL enforce choice terminal outcomes with deterministic mapping to
ACP action and sender-visible terminal outcome/reason_code.

Mapping:

- `selected` → send ACP selected outcome with the chosen `option_id`; prompt
  continues under existing ACP stop-reason mapping contract
- `cancelled` → send ACP cancelled outcome; sender-visible terminal outcome
  `failed` with `reason_code=runtime_choices_request_cancelled`

For raww requests with pending ACP choice turns, the queued response is already
immutable; the sender receives the terminal `delivery_outcome` event only after
the choice is resolved.

#### Scenario: Map cancelled choice to failed terminal outcome

- **WHEN** choice decision resolves to cancelled
- **THEN** relay sends ACP cancelled
- **AND** sender-visible terminal outcome is `failed`
- **AND** `reason_code = runtime_choices_request_cancelled`

#### Scenario: Cancelled choice yields failed delivery_outcome

- **WHEN** relay already returned queued response for an ACP raww
- **AND** the pending ACP choice later resolves to cancelled
- **THEN** relay emits a `delivery_outcome` event with `outcome = "failed"`
- **AND** `reason_code = runtime_choices_request_cancelled`

### Requirement: ACP Choice Option Fidelity

Relay SHALL preserve ACP permission-option fidelity for operator decisioning.

Normative reference:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

Conformance note:
- Implementers MUST read and conform to ACP `session/request_permission`
  semantics from the Tool Calls spec before modifying relay choice logic.

Decision contract:

- `choices.pick` SHALL include `outcome`
- allowed decision outcomes are `selected` and `cancelled`
- `selected` SHALL include explicit `option_id`
- `cancelled` SHALL NOT include `option_id`
- relay MUST reject invalid outcome/option combinations with
  `validation_invalid_params`
- relay MUST reject decisions with unknown/non-pending option IDs using
  deterministic validation/runtime taxonomy

Lifecycle payload contract:

- `choices.requested` payload SHALL include ACP option metadata needed for UI
  rendering and explicit option selection

#### Scenario: Resolve with explicit option id from UI decision

- **WHEN** UI submits `choices.pick` with `outcome=selected` and explicit
  `option_id`
- **THEN** relay uses the supplied `option_id` for ACP selected outcome
- **AND** does not transform or substitute the selected option id

#### Scenario: Reject decision missing option id

- **WHEN** UI submits `choices.pick` with `outcome=selected` and missing
  `option_id`
- **THEN** relay rejects with `validation_invalid_params`

### Requirement: Choice Decision Arbitration

First authorized decision SHALL win across both `ui` and `operator`
submitters. Subsequent decisions on resolved requests SHALL be rejected with
`runtime_choices_request_already_resolved` and SHALL NOT mutate state.

#### Scenario: Reject late decision after prior approval

- **WHEN** a second authorized submitter (ui or operator) decides an already
  resolved request
- **THEN** relay rejects with `runtime_choices_request_already_resolved`

### Requirement: Choice Decision Denial Schema

When relay denies choice decisioning by policy, relay SHALL return
`authorization_forbidden` with canonical minimum details:

- `capability`
- `requester_session`
- `bundle_name`
- `reason`

Optional additive details MAY include `target_session`, `targets`,
`policy_rule_id`, and ACP-specific metadata.

The denial schema applies uniformly to `client_class=ui` and
`client_class=operator` submitters.

#### Scenario: Return canonical denial details for unauthorized decision submitter

- **WHEN** a `{ui, operator}` principal lacks `choose` capability
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include canonical required fields

### Requirement: Operator Client Class

Relay SHALL recognize `operator` as a stream `client_class` distinct from
`agent` and `ui`.

Operator class is a decision-submitter role in alpha:

- operator-class streams MAY submit `choices.pick` decisions and
  `choices.list` queries,
- operator-class streams SHALL NOT be inbound delivery targets,
- operator-class streams SHALL NOT receive `choices.snapshot`,
  `choices.requested`, or `choices.resolved` push events; push events
  remain UI-only in alpha.

#### Scenario: Operator class admitted as distinct from agent and ui

- **WHEN** relay enumerates supported stream client classes
- **THEN** the supported set is `{agent, ui, operator}`

### Requirement: Operator-Class Policy Authorization

Bundle policy preset SHALL be the sole source of authority for whether a
configured session may register with `client_class=operator`.

Operator-class authorization SHALL be evaluated at hello time only. Decision
authority (`choose` capability) is evaluated independently per request.

If a configured session attempts `hello` with `client_class=operator` but the
bundle policy preset does not authorize operator class for that principal,
relay SHALL reject with `validation_invalid_client_class_for_hello`.

Operator-class authorization and `choose` capability SHALL remain independent
gates; both must be satisfied for a session to resolve choice requests.

#### Scenario: Reject operator hello when policy preset lacks operator-class authorization

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset for that principal does not authorize
  operator-class registration
- **THEN** relay rejects with `validation_invalid_client_class_for_hello`

#### Scenario: Operator hello accepted without choose capability

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset authorizes operator-class registration for
  that principal
- **AND** the policy `choose` capability is `none`
- **THEN** relay accepts the hello
- **AND** subsequent `choices.pick` from that stream is rejected with
  `authorization_forbidden`

### Requirement: Choice List Polling Request

Relay SHALL accept `RelayRequest::ChoicesList` from associated principals
with `client_class ∈ {ui, operator}` and `choose` capability satisfying policy.

`ChoicesList` returns the current set of pending choice requests for
the requester's bundle. The bundle scope is derived from the request's routing
namespace; no caller-supplied bundle selector is accepted.

Response payload SHALL include for each pending request the same field set
emitted by `choices.requested` events:

- `message_id`
- `choice_request_id`
- `target_session`
- `requested_kind`
- `requested_details` (including ACP option metadata)
- `enqueued_at`

Response SHALL include a `schema_version` field and a top-level array of
pending records ordered by enqueue `sequence` ascending.

`ChoicesList` SHALL NOT mutate queue state.

Push events (`choices.snapshot`, `choices.requested`, `choices.resolved`)
remain UI-only in alpha. Operator-class visibility is poll-only via `ChoicesList`.

#### Scenario: Operator client lists pending choice requests

- **WHEN** an operator-class principal with `choose` capability submits
  `ChoicesList`
- **THEN** relay returns pending records in FIFO `sequence` order
- **AND** each record contains the `choices.requested` field set

#### Scenario: Reject choice list from agent class

- **WHEN** a principal with `client_class=agent` submits `ChoicesList`
- **THEN** relay rejects with `validation_invalid_client_class_for_action`

#### Scenario: Reject choice list from operator without choose capability

- **WHEN** an operator-class principal without `choose` capability submits
  `ChoicesList`
- **THEN** relay rejects with `authorization_forbidden`
- **AND** denial details include `capability="choose"`

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
at the `home` tier under the Uniform Cross-Bundle Authorization Model and
never demands `all`. This holds whether the sender is bundle-bound (an
agent replying to the operator) or itself relay-wide (one relay-wide principal
messaging another). It is asymmetric with a relay-wide *requester* reaching
*into* a bundle, which the uniform model classifies as cross-namespace and which
therefore does require `all`.

#### Scenario: Bundle-bound agent sends to @GLOBAL operator

- **WHEN** a session principal sends `Send` with
  `targets = ["operator@GLOBAL"]`
- **AND** `operator@GLOBAL` is registered as a relay-wide session
- **THEN** relay delivers the message to `operator@GLOBAL`

#### Scenario: @GLOBAL principal sends to bundle session

- **WHEN** a relay-wide principal sends `Send` with
  `targets = ["agent@bundle-a"]`
- **THEN** relay routes to bundle `bundle-a` and delivers to `agent`

#### Scenario: @GLOBAL principal rawws to bundle session under all

- **WHEN** a relay-wide principal issues a Raww request with
  `target_session = "agent@bundle-a"`
- **AND** the requester's configured `raww` scope is `all`
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

### Requirement: GLOBAL Namespace List

The relay SHALL return the set of currently registered relay-wide sessions
when `List` is requested with `namespace = "GLOBAL"`.

#### Scenario: List relay-wide sessions

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** one or more relay-wide sessions are currently registered
- **THEN** relay returns `RelayResponse::List` containing those sessions

#### Scenario: List with no relay-wide sessions registered

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** no relay-wide sessions are currently registered
- **THEN** relay returns `RelayResponse::List` with an empty session set

### Requirement: Retire GLOBAL Routing Stub

The relay SHALL NOT return `validation_namespace_routing_unavailable`. This
temporary error code is retired when suffix-based GLOBAL routing is
implemented.

#### Scenario: @GLOBAL target no longer returns stub error

- **WHEN** a session sends `Send` with an `@GLOBAL` target
- **THEN** the relay SHALL NOT return `validation_namespace_routing_unavailable`
- **AND** SHALL route or return an appropriate typed error per the suffix
  routing rules above

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

### Requirement: Dynamic Bundle File Watching

The relay host SHALL watch the bundles configuration directory for filesystem
changes when started without `--no-watch`. The watcher SHALL use a debounced
reconcile-on-change model: on any debounced notification the relay re-scans
the full directory and reconciles the loaded bundle set against the on-disk
set. Debounce window SHALL be short enough for interactive use (~200ms) and
long enough to avoid acting on partial writes.

On a new bundle file: relay SHALL load and start the bundle runtime,
equivalent to `bundle up`. Validation errors on the new file are recorded as
startup failures; the relay continues serving other bundles unchanged.

On a disappeared bundle file: relay SHALL emit a typed error frame
(`runtime_bundle_unloaded`) to every active session in that bundle, close all
connections for that bundle, and unload the bundle from the runtime catalog.
Principal store entries for the affected sessions SHALL be retained (the
principal store is relay-level and not pruned on bundle unload).

On a modified bundle file: relay SHALL treat the change as a disappear followed
by a new file: emit `runtime_bundle_reloaded`, disconnect all active sessions,
then reload the bundle with the new configuration.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the bundles directory
- **AND** the relay is running with watching enabled (default)
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: Bundle file removed at runtime with active sessions

- **WHEN** a bundle TOML file is removed from the bundles directory
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before disconnect
- **AND** closes all connections for that bundle
- **AND** unloads the bundle from the runtime catalog
- **AND** subsequent connection attempts for that bundle return `validation_unknown_bundle`

#### Scenario: Bundle file modified at runtime with active sessions

- **WHEN** a bundle TOML file is modified
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watch disabled — no runtime reconcile

- **WHEN** the relay was started with `--no-watch`
- **AND** a bundle file is added, removed, or modified at runtime
- **THEN** relay does NOT reconcile the bundle set
- **AND** changes take effect only after relay restart

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's identity and policy controls in the
requester's **home namespace** — its bound bundle's policy for a session
principal, or the operator policy for a relay-wide principal — classify the
requester-to-target relationship relative to that home namespace, and require a
scope tier on the policy scope ladder:

- self target → `self`
- same-namespace non-self target → `home`
- other-namespace target → `all`

A principal's home namespace SHALL be its native namespace: a session's home is
its bundle, and a relay-wide principal's home is its reserved namespace
(`GLOBAL` / `EXTERNAL` / `RELAY`). `home` SHALL therefore confer authority
only within the principal's own namespace; a relay-wide principal (for example a
`@GLOBAL` operator) SHALL require `all` to reach into any bundle, since a
bundle is not its home namespace. There SHALL be no global/relay-principal
exemption from this threshold.

This requester-axis rule has a target-axis counterpart: a relay-wide
(`@GLOBAL`) *target* SHALL classify at the `home` tier rather than `all`,
because relay-wide principals are delivered through the session registry rather
than by crossing into a peer bundle (see Suffix-Based Target Routing). Reaching a
relay-wide target — an agent messaging the operator, or one relay-wide principal
messaging another — is therefore not a cross-namespace act and SHALL NOT demand
`all`. This is a routing invariant, not a per-operation policy exemption.

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-namespace policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer namespace
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer namespace SHALL NOT be required, and the
relay SHALL NOT resolve or authorize the requester in a target's (or any other
borrowed) bundle in place of its home namespace, on any target operation.

Whether a capability can be configured to a cross-bundle (`all`) scope SHALL
be governed by the policy schema's per-capability allowed-scope set, not by
relay routing code. A capability whose schema cap is below `all` SHALL
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

Every target reachable via look or raww SHALL have transport capabilities
derivable from its `SessionType` at check time:

- `can_be_looked: bool` — the session can be targeted by `look` (its transport
  supports snapshot capture)
- `can_be_written: bool` — the session can be targeted by `raww` (its transport
  supports raw input injection)
- `can_stream_output: bool` — the session's transport natively produces live
  output chunks (ACP and PTY stream output natively; Tmux requires periodic
  polling)
- `can_give_choices: bool` — the session's transport can surface choice requests
  (the transport produces ACP-style option arrays for operator/UI resolution).
  Describes choice *production*, not resolution authority — any session with
  sufficient `choose` policy scope may resolve choices regardless of its own
  `can_give_choices` value. (Renamed from `gives_choices` to align with the
  `can_` prefix convention established by `add-transport-capability-flags`.)

Capabilities SHALL be derived from the target's `SessionType` (from
`BundleMember` configuration for bundle targets, or from `TuiSession::session_type`
in `users.toml` for relay-wide targets):

| Transport | `can_be_looked` | `can_be_written` | `can_stream_output` | `can_give_choices` |
|-----------|----------------|-----------------|--------------------|--------------------|
| `Tmux`    | true           | true            | false              | false              |
| `Acp`     | true           | true            | true               | true               |
| `Pty`     | true           | true            | true               | false              |
| `Ui`      | false          | false           | false              | false              |
| `Pubsub`  | false          | false           | false              | false              |

`can_stream_output` is advertised on registration; streaming look semantics
that consume it are deferred to a follow-on proposal.

When a look or raww operation resolves a target whose transport type has the
relevant capability false, relay SHALL return `validation_unsupported_operation`.
This check precedes authorization policy checks and applies to both bundle
targets (checked from `BundleMember` configuration) and relay-wide targets
(checked from `users.toml` session type).

#### Scenario: Reject look against session with can_be_looked false

- **WHEN** a `look` request resolves to a target whose transport type carries
  `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Reject raww against session with can_be_written false

- **WHEN** a `raww` request resolves to a target whose transport type carries
  `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Permit look against session with can_be_looked true

- **WHEN** a `look` request resolves to a target whose transport type carries
  `can_be_looked = true`
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: Permit raww against session with can_be_written true

- **WHEN** a `raww` request resolves to a target whose transport type carries
  `can_be_written = true`
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: ACP session advertises can_give_choices true

- **WHEN** an ACP-backed session registers with the relay
- **THEN** its registered capabilities include `can_give_choices = true`

#### Scenario: Tmux session advertises can_give_choices false

- **WHEN** a Tmux-backed session registers with the relay
- **THEN** its registered capabilities include `can_give_choices = false`

