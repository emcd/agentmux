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
