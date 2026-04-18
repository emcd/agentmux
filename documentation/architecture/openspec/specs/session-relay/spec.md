# session-relay Specification

## Purpose
TBD - created by archiving change add-mcp-session-relay-mvp. Update Purpose after archive.
## Requirements
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
  - required `coder` reference
  - optional `coder-session-id`

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present
- each session `coder` references an existing coder id from `coders.toml`

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
  - for `channel = "stdio"`:
    - required `command`
  - for `channel = "http"`:
    - required `url`
    - optional `headers` entries (`name`, `value`)

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session operation and
  SHALL NOT silently fall back to ACP `session/new` in the same operation.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid v2 tmux coder + session configuration

- **WHEN** bundle file uses `format-version = 2`
- **AND** coders file uses `format-version = 2`
- **AND** a coder defines `[coders.tmux]` with required fields
- **AND** sessions use unique `id` values
- **AND** optional session `name` values are unique when present
- **AND** each session references an existing coder
- **THEN** the system loads configuration successfully

#### Scenario: Load valid v2 ACP stdio coder + session configuration

- **WHEN** bundle and coders files use `format-version = 2`
- **AND** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is provided
- **AND** sessions use unique `id` values
- **AND** optional session `name` values are unique when present
- **AND** each session references an existing coder
- **THEN** the system loads configuration successfully

#### Scenario: Reject unknown coder reference

- **WHEN** a session references a `coder` value not present in `coders.toml`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session id in one bundle

- **WHEN** one bundle contains duplicate session `id` values
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session name in one bundle

- **WHEN** one bundle contains duplicate session `name` values
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject missing coder target descriptor

- **WHEN** a coder omits both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject multiple coder target descriptors

- **WHEN** a coder defines both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Select ACP session load when session identity is present

- **WHEN** a session references an ACP coder
- **AND** the session includes `coder-session-id`
- **THEN** runtime selects ACP `session/load` for that session.

#### Scenario: Fail fast when ACP session load fails

- **WHEN** runtime selects ACP `session/load` for a session
- **AND** the ACP `session/load` call returns an error
- **THEN** runtime fails the session operation
- **AND** runtime does not call ACP `session/new` as fallback in the same
  operation.

#### Scenario: Reject ACP stdio channel without command

- **WHEN** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is missing
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject unsupported format-version

- **WHEN** bundle or coders file uses `format-version` other than `2`
- **THEN** the system rejects configuration with a validation error

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
The system SHALL resolve each target session to its currently active pane at
delivery time.
The system SHALL support directed delivery to one or more explicitly selected
target sessions.

For send explicit targets, relay SHALL accept only canonical target
identifiers:

- configured bundle member `session_id`,
- configured/registered UI session id (when UI routing is supported).

Relay SHALL NOT resolve configured bundle session `name` values as send-target
aliases.
Session `name` remains informational metadata only and is not send-routable.

When one explicit token exactly matches both a bundle member `session_id` and a
UI session id, relay SHALL route to the bundle member `session_id`.

#### Scenario: Resolve session target for direct send

- **WHEN** a caller sends a message to one target session id
- **THEN** the system routes by that session id
- **AND** resolves the session's active pane as the concrete tmux injection
  endpoint

#### Scenario: Reject configured name alias as explicit send target

- **WHEN** a caller sends a message using a configured session `name` token
- **THEN** relay rejects the target with `validation_unknown_target`

#### Scenario: Prefer bundle member session_id on overlap with UI session id

- **WHEN** an explicit token matches both bundle member `session_id` and UI
  session id
- **THEN** relay routes to the bundle member target

#### Scenario: Active pane changes before delivery

- **WHEN** the active pane for a target session changes before injection
- **THEN** the system resolves the new active pane at delivery time
- **AND** injects into that resolved pane

#### Scenario: Broadcast to all known bundle sessions

- **WHEN** a caller sends a broadcast message
- **THEN** the system attempts delivery to every known session in the bundle
  except the sender session

#### Scenario: Deliver to explicit target subset

- **WHEN** a caller sends one message to a selected subset of sessions
- **THEN** the system attempts delivery only to those selected sessions
- **AND** does not deliver to other known bundle sessions

### Requirement: JSON Chat Envelope

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

The default quiescence values for `sync` delivery SHALL be:

- `quiet_window_ms = 750`
- `delivery_timeout_ms = 30000`

For `async` delivery, relay SHALL keep accepted targets pending and wait
indefinitely for quiescence before injection.

When request-level `quiescence_timeout_ms` is provided, relay SHALL use that
value as the wait bound for both modes.

Request-level `quiescence_timeout_ms` SHALL map to relay's effective delivery
wait timeout for the request.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the system injects the pending message

#### Scenario: Use default quiescence values in sync mode

- **WHEN** a caller requests `delivery_mode=sync`
- **AND** does not provide quiescence configuration overrides
- **THEN** the system uses `quiet_window_ms = 750`
- **AND** uses `delivery_timeout_ms = 30000`

#### Scenario: Time out while waiting for quiescence in sync mode

- **WHEN** `delivery_mode=sync`
- **AND** pane output keeps changing until the delivery timeout elapses
- **THEN** the system reports target delivery failure with timeout reason
- **AND** does not inject the message for that target

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** `delivery_mode=async`
- **AND** pane output continues changing beyond sync timeout thresholds
- **THEN** the system keeps the target pending
- **AND** attempts injection after a future quiescent window is observed

#### Scenario: Apply request quiescence timeout override in async mode

- **WHEN** `delivery_mode=async`
- **AND** request provides `quiescence_timeout_ms`
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

The system SHALL support both asynchronous acceptance responses and
synchronous completion responses, and SHALL NOT require accept/done
acknowledgements.

#### Scenario: Report accepted async delivery

- **WHEN** relay accepts an `async` chat request for one or more targets
- **THEN** the immediate result marks those targets as `queued`
- **AND** does not wait for final delivery outcome before responding

#### Scenario: Report successful sync tmux injection

- **WHEN** relay processes a `sync` chat request and tmux injection succeeds for
  a target
- **THEN** the result marks that target as delivered to pane input
- **AND** includes the `message_id` and target session name

#### Scenario: Report failed sync tmux injection

- **WHEN** relay processes a `sync` chat request and tmux injection fails for a
  target
- **THEN** the result marks that target as failed
- **AND** includes a failure reason

#### Scenario: Return no-op completion for zero effective targets

- **WHEN** sender exclusion and target resolution produce zero effective
  recipients
- **THEN** relay returns an immediate no-op response without validation error
- **AND** response contains zero per-target results

### Requirement: MVP Trust Boundary

The system SHALL operate in a same-host, same-user trust boundary for MVP.
Authorization principal identity SHALL be association/socket-driven requester
identity resolved by runtime context.
Caller-supplied sender-like payload fields SHALL NOT override principal
identity.

#### Scenario: Operate against current user's tmux server

- **WHEN** delivery or reconciliation executes
- **THEN** the system targets tmux resources owned by the current host user

#### Scenario: Ignore caller-supplied sender override for principal identity

- **WHEN** caller supplies sender-like payload field that conflicts with
  associated requester identity
- **THEN** relay authorizes against associated requester identity
- **AND** does not treat payload override as authoritative

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
- `target_session` (required)
- `lines` (optional)
- `bundle_name` (optional/redundant when bundle is already bound by
  association/socket context)

MVP authorization posture for `look` SHALL be:

- default scope `self`
- broader scope controlled by policy (`all:home` or `all:all`)
- cross-bundle look currently unsupported by runtime contract

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request omits `bundle_name`
- **THEN** relay resolves bundle from associated runtime context

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Reject mismatched bundle name in MVP

- **WHEN** look request includes `bundle_name` that does not match
  associated runtime context
- **THEN** relay rejects request with `validation_cross_bundle_unsupported`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`

### Requirement: Look Capture Window Bounds

Look capture window SHALL use deterministic bounds:

- default `lines = 120`
- maximum `lines = 1000`
- valid range `1..=1000`

#### Scenario: Apply default line window

- **WHEN** look request omits `lines`
- **THEN** relay captures using default `lines = 120`

#### Scenario: Reject out-of-range line window

- **WHEN** look request includes `lines` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_lines`

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

`snapshot_lines` ordering SHALL be oldest-to-newest.

#### Scenario: Return canonical look payload

- **WHEN** look succeeds
- **THEN** relay returns canonical look response payload
- **AND** `snapshot_lines` contains ordered snapshot lines from oldest to
  newest

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

Each client stream SHALL begin with `hello` registration frame containing:

- `bundle_name`
- `session_id`
- `client_class` (`agent` | `ui`)

`hello` identity SHALL bind principal/session for that stream using canonical
identity key:

- `(bundle_name, session_id)`

For `client_class=agent`, `session_id` SHALL resolve via bundle
`[[sessions]]` configuration.

For `client_class=ui`, `session_id` SHALL resolve via global TUI sessions from
`<config-root>/tui.toml`.

If a second stream attempts `hello` for the same identity while the current
owner is still live, relay SHALL reject second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for configured UI session identity

- **WHEN** TUI client sends valid `hello` with `client_class=ui`
- **AND** `session_id` maps to configured global TUI session `id`
- **THEN** relay accepts hello and binds stream owner identity

#### Scenario: Reject hello for unknown UI session identity

- **WHEN** a stream sends `hello` with `client_class=ui`
- **AND** `session_id` is not present in global TUI sessions
- **THEN** relay rejects hello with `validation_unknown_sender`

### Requirement: Same-Bundle Stream Scope Enforcement

Persistent stream routing in MVP SHALL remain same-bundle only.

Relay SHALL reject cross-bundle stream/request attempts with
`validation_cross_bundle_unsupported`.

#### Scenario: Reject cross-bundle request frame

- **WHEN** a registered stream submits request frame scoped to bundle that does
  not match stream identity bundle
- **THEN** relay rejects with `validation_cross_bundle_unsupported`

### Requirement: Static Recipient Routability

Static configured recipients from bundle session definitions SHALL remain
routable independent of active stream presence or prior `hello` from those
recipients.

#### Scenario: Route to configured recipient before recipient stream registration

- **WHEN** sender targets a configured bundle recipient
- **AND** recipient has no active stream registration
- **THEN** relay processes routing using configured recipient identity semantics
- **AND** does not reject solely for missing recipient `hello`

### Requirement: Endpoint Class Routing Behavior

Relay SHALL route recipient delivery by endpoint class:

- `agent` recipients use existing prompt-injection/quiescence delivery path
- `ui` recipients use stream push event delivery path

For disconnected `ui` recipients, relay SHALL keep pending delivery queued using
existing relay async queue machinery and attempt delivery when same identity
reconnects.

Endpoint class resolution SHALL be deterministic with this precedence:

1. active `hello` registration for target identity
2. otherwise, target configured in bundle with no active registration defaults
   to class `agent`
3. otherwise, target is rejected as unknown

Recipient-class transport matrix in MVP SHALL be:

- `agent`: prompt-injection/quiescence path; active stream binding not required
  for routability
- `ui` with active binding: stream push event path
- `ui` without active binding: queue and retry on reconnect

Non-UI stream-recipient classes are an empty set in MVP.
Therefore, no-live-binding fail-fast rules for non-UI stream recipients are
non-operative in MVP and reserved for a future class expansion.

#### Scenario: Deliver to agent recipient via prompt injection path

- **WHEN** target recipient is class `agent`
- **THEN** relay uses existing prompt-injection/quiescence delivery behavior

#### Scenario: Deliver to connected ui recipient via stream event

- **WHEN** target recipient is class `ui`
- **AND** recipient has active registered stream
- **THEN** relay emits inbound-message event frame to that stream

#### Scenario: Queue ui delivery while stream is disconnected

- **WHEN** target recipient is class `ui`
- **AND** recipient has no active registered stream
- **THEN** relay keeps pending delivery queued
- **AND** attempts delivery when same identity reconnects

#### Scenario: Default unregistered configured recipient to agent class

- **WHEN** target recipient is configured in bundle
- **AND** target has no active registration
- **THEN** relay resolves endpoint class as `agent`

#### Scenario: Reject unregistered unknown recipient

- **WHEN** target has no active registration
- **AND** target is not configured in associated bundle
- **THEN** relay rejects request with `validation_unknown_recipient`

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `bundle_name`
- `target_session`
- `created_at`

MVP event types SHALL include:

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
  - `list = all:home`
  - `look = self`
  - `send = all:home`
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

- `find`: `self` | `all:home` | `all:all`
- `list`: `all:home` | `all:all`
- `look`: `self` | `all:home` | `all:all`
- `send`: `all:home` | `all:all`
- `do`: map `action_id -> (none | self | all:home | all:all)`

For current self-target-only `do` MVP behavior:

- `none` and `self` are operative
- `all:home` and `all:all` are reserved/non-operative until non-self `do`
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
- **AND** action scope is `all:home` or `all:all`
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

#### Scenario: Deny list_sessions without successful payload

- **WHEN** requester identity is valid
- **AND** policy denies `list.read` for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return a successful `bundle.sessions[]` payload

### Requirement: Relay Send Scope Control

Relay send authorization SHALL be driven by `send` control scope:

- `all:home` allows only same-bundle targets
- `all:all` allows cross-bundle targets when runtime support/trust path exists

#### Scenario: Reject cross-bundle send under home-only scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all:home"`
- **THEN** relay returns `authorization_forbidden`

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

Relay look SHALL support ACP-backed target sessions using relay-managed
snapshot state populated from ACP prompt-turn updates.

For ACP targets, relay SHALL:
- ingest non-empty text lines from ACP `session/update` payloads during
  prompt turns
- persist those lines in per-session runtime state
- retain at most 1000 lines per session
- evict oldest lines first when retention exceeds 1000
- return look results ordered oldest -> newest
- return tail lines based on requested `lines`
- return success with `snapshot_lines = []` when no retained snapshot exists

#### Scenario: Return ACP look snapshot from retained updates

- **WHEN** requester invokes relay `look` for a target session backed by ACP
  transport after ACP prompt turns emitted `session/update` text
- **THEN** relay returns successful look payload with retained `snapshot_lines`
- **AND** `snapshot_lines` are ordered oldest -> newest

#### Scenario: Enforce bounded ACP look retention and oldest-first eviction

- **WHEN** retained ACP snapshot lines for one target exceed 1000
- **THEN** relay evicts oldest lines first
- **AND** subsequent look requests return at most 1000 retained lines

#### Scenario: Return empty ACP look snapshot when no update lines exist

- **WHEN** requester invokes relay `look` for ACP target with no retained
  snapshot state
- **THEN** relay returns successful look payload with `snapshot_lines = []`

#### Scenario: Preserve existing tmux look behavior unchanged

- **WHEN** requester invokes relay `look` for a target session backed by tmux
  transport
- **THEN** relay executes canonical look capture behavior unchanged

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

### Requirement: ACP Transport Timeout Semantics

ACP-backed send operations SHALL use turn-wait timeout semantics rather than
pane-quiescence semantics.

For ACP targets:

- request-level `acp_turn_timeout_ms` SHALL apply as ACP turn-wait timeout
- coder-level `[coders.acp] turn-timeout-ms` SHALL provide default timeout
- if neither value is set, system default SHALL be `120000` ms
- precedence SHALL be:
  1. request `acp_turn_timeout_ms`
  2. coder `[coders.acp] turn-timeout-ms`
  3. system default `120000`

Transport-field validation SHALL be fail-fast:

- ACP target + `quiescence_timeout_ms` =>
  `validation_invalid_timeout_field_for_transport`
- tmux target + `acp_turn_timeout_ms` =>
  `validation_invalid_timeout_field_for_transport`
- request includes both timeout fields =>
  `validation_conflicting_timeout_fields`

#### Scenario: Apply request ACP timeout override

- **WHEN** a send request to ACP target includes `acp_turn_timeout_ms`
- **THEN** relay uses that value as ACP turn-wait timeout for that target

#### Scenario: Fall back to coder ACP timeout default

- **WHEN** a send request to ACP target omits `acp_turn_timeout_ms`
- **AND** coder defines `[coders.acp] turn-timeout-ms`
- **THEN** relay uses coder timeout value for that target

#### Scenario: Reject quiescence timeout for ACP target

- **WHEN** request targets ACP transport
- **AND** request includes `quiescence_timeout_ms`
- **THEN** relay returns `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout for tmux target

- **WHEN** request targets tmux transport
- **AND** request includes `acp_turn_timeout_ms`
- **THEN** relay returns `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject conflicting timeout fields

- **WHEN** request includes `quiescence_timeout_ms` and `acp_turn_timeout_ms`
- **THEN** relay returns `validation_conflicting_timeout_fields`

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

### Requirement: ACP Sync Delivery Phase Contract

For `delivery_mode=sync` and ACP targets, relay SHALL use a two-phase contract.

Phase 1 (delivery acknowledgment):

- relay SHALL report target `outcome=delivered` when first ACP activity is
  observed (`session/update` notification or prompt result)
- phase-1 response SHALL include
  `details.delivery_phase = "accepted_in_progress"`

Phase 2 (terminal completion):

- terminal prompt completion SHALL drive relay-internal worker readiness state
- phase-2 completion SHALL NOT retroactively mutate phase-1 sync response
- phase-2 completion SHALL NOT be required sender-facing `send` output in MVP

#### Scenario: Return delivered on first ACP activity

- **WHEN** sync send targets ACP session
- **AND** relay observes first ACP activity before terminal completion
- **THEN** relay returns target `outcome=delivered`
- **AND** includes `details.delivery_phase = "accepted_in_progress"`

#### Scenario: Fail before first ACP activity

- **WHEN** sync send targets ACP session
- **AND** ACP transport fails before first activity is observed
- **THEN** relay returns terminal failure/timeout outcome for that target

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals to maintain internal worker
readiness state for scheduling.

MVP state model:

- `available`: worker healthy and ready for next prompt
- `busy`: prompt accepted and turn in progress
- `unavailable`: worker transport/process failure requiring restart

Transition contract:

- first ACP activity observed => `busy`
- terminal stopReason observed => `available`
- disconnect/error requiring restart => `unavailable`

MVP sender-surface contract:

- these transitions SHALL NOT require additional sender-facing `send` outputs
- send success semantics remain phase-1 delivery acknowledgment only

#### Scenario: Mark worker available on terminal stopReason

- **WHEN** ACP worker reports terminal stopReason for in-progress prompt
- **THEN** relay marks worker state as `available`
- **AND** subsequent sends MAY be admitted for that target

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends.

Worker model SHALL be:

- one worker per target session
- serialized request queue per worker
- fixed MVP queue bound `max_pending = 64`

Backpressure contract:

- enqueue beyond bound SHALL fail with `runtime_acp_queue_full`

Disconnect/restart contract:

- disconnect before phase-1 acknowledgment =>
  `runtime_acp_connection_closed`
- disconnect after phase-1 acknowledgment SHALL keep response immutable and
  transition worker to `unavailable` for recovery

Restart sequence SHALL be:

1. spawn ACP process
2. initialize
3. select lifecycle (`session/load` when identity exists, else `session/new`)
4. prompt

Failure taxonomy SHALL include:

- `runtime_acp_initialize_failed`
- `runtime_acp_session_load_failed`
- `runtime_acp_session_new_failed`
- `runtime_acp_prompt_failed`
- `acp_turn_timeout`

#### Scenario: Reject enqueue beyond fixed queue bound

- **WHEN** ACP worker queue depth reaches `max_pending`
- **AND** relay receives another ACP send for same target
- **THEN** relay returns `runtime_acp_queue_full`

#### Scenario: Surface disconnect before phase-1 acknowledgment

- **WHEN** ACP worker disconnects before first activity is observed
- **THEN** relay reports `runtime_acp_connection_closed`

### Requirement: ACP Permission Request Readiness Signal (MVP)

Relay SHALL treat ACP `session/request_permission` as in-progress turn activity
for ACP readiness tracking in MVP.

MVP behavior contract:

- `session/request_permission` observed before terminal completion SHALL count
  as first activity for two-phase sync acknowledgment semantics
- worker readiness SHALL transition to `busy` while turn completion remains
  pending
- terminal stopReason completion SHALL transition readiness to `available`

MVP boundary:

- this change does not lock permission allow/deny decisioning behavior
- this change does not lock ACP permission timeout/error taxonomy

#### Scenario: Treat permission request as first ACP activity

- **WHEN** relay observes ACP `session/request_permission` before prompt result
- **THEN** sync send MAY return phase-1 `outcome=delivered`
- **AND** includes `details.delivery_phase = "accepted_in_progress"`

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
