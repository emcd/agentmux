## MODIFIED Requirements

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

## ADDED Requirements

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
