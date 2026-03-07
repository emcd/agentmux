# session-relay Specification

## Purpose
TBD - created by archiving change add-mcp-session-relay-mvp. Update Purpose after archive.
## Requirements
### Requirement: Bundle Membership Configuration

The system SHALL let operators define a bundle as a set of session members in
configuration, where each member includes:

- session id
- optional session name
- working directory
- coder start command

#### Scenario: Load valid configured bundle

- **WHEN** a configured bundle definition contains unique session ids
- **AND** optional session names are unique when present
- **THEN** the system loads the bundle definition successfully

#### Scenario: Reject duplicate session names in one bundle

- **WHEN** a configured bundle definition contains duplicate non-empty session names
- **THEN** the system rejects the bundle definition with a validation error

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
The default quiescence values SHALL be:

- `quiet_window_ms = 750`
- `delivery_timeout_ms = 30000`

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the system injects the pending message

#### Scenario: Use default quiescence values

- **WHEN** a caller does not provide quiescence configuration overrides
- **THEN** the system uses `quiet_window_ms = 750`
- **AND** uses `delivery_timeout_ms = 30000`

#### Scenario: Time out while waiting for quiescence

- **WHEN** pane output keeps changing until the delivery timeout elapses
- **THEN** the system reports target delivery failure with timeout reason
- **AND** does not inject the message for that target

### Requirement: Quiescence Documentation

The system SHALL document quiescence constraints and known interference
patterns for users configuring agent sessions.

#### Scenario: Document dynamic output caveat

- **WHEN** project documentation is generated for the relay capability
- **THEN** it includes a warning that continuously changing output sources
  (for example clock-style statusline content) can prevent quiescence
  detection from succeeding

### Requirement: Delivery Results Without ACK Protocol

The system SHALL return synchronous per-target delivery results from MCP
operations and SHALL NOT require accept/done acknowledgements in MVP.

#### Scenario: Report successful tmux injection

- **WHEN** tmux injection succeeds for a target
- **THEN** the result marks that target as delivered to pane input
- **AND** includes the `message_id` and target session name

#### Scenario: Report failed tmux injection

- **WHEN** tmux injection fails for a target
- **THEN** the result marks that target as failed
- **AND** includes a failure reason

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
