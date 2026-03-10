# session-relay Specification

## Purpose
TBD - created by archiving change add-mcp-session-relay-mvp. Update Purpose after archive.
## Requirements
### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version`
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - `coder`
  - optional `coder-session-id`

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid TOML bundle configuration

- **WHEN** target `bundles/<bundle-id>.toml` contains unique session IDs
- **AND** optional session `name` values are unique when present
- **AND** each session `coder` references an existing coder ID from
  `coders.toml`
- **THEN** the system loads the bundle definition successfully

#### Scenario: Reject unknown coder reference

- **WHEN** a session references a `coder` value not present in `coders.toml`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session name in one bundle

- **WHEN** one bundle contains duplicate session `name` values
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
The system SHALL additionally allow explicit targets to reference configured
session names as aliases.

#### Scenario: Resolve session target for direct send

- **WHEN** a caller sends a message to one target session id
- **THEN** the system routes by that session id
- **AND** resolves the session's active pane as the concrete tmux injection
  endpoint

#### Scenario: Resolve session target by configured name alias

- **WHEN** a caller sends a message using a configured session name
- **THEN** the system resolves that name to one session id
- **AND** routes delivery to that resolved id

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

#### Scenario: Operate against current user's tmux server

- **WHEN** delivery or reconciliation executes
- **THEN** the system targets tmux resources owned by the current host user

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

This field applies to bundle-level relay host grouping and SHALL NOT change
session routing identity semantics.

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

