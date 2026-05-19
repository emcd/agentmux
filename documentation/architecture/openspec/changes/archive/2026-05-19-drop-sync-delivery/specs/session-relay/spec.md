## MODIFIED Requirements

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

### Requirement: Delivery Results Without ACK Protocol

Relay SHALL use asynchronous acceptance responses and SHALL NOT support
synchronous completion responses.

An accepted send request SHALL return immediately with per-target `outcome =
queued`. Relay SHALL NOT block the caller waiting for delivery completion.

#### Scenario: Report accepted async delivery

- **WHEN** relay accepts a chat request for one or more targets
- **THEN** the immediate result marks those targets as `queued`
- **AND** does not wait for final delivery outcome before responding

#### Scenario: Return no-op completion for zero effective targets

- **WHEN** sender exclusion and target resolution produce zero effective
  recipients
- **THEN** relay returns an immediate no-op response without validation error
- **AND** response contains zero per-target results

## REMOVED Requirements

### Requirement: ACP Sync Delivery Phase Contract
