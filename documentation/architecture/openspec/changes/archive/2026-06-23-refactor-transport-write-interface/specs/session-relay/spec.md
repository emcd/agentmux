## MODIFIED Requirements

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing. Quiescence gating is transport-internal: each transport that
supports quiescence (Tmux today) SHALL wait for the target to become idle before
flushing its internal write buffer. The relay delivery worker SHALL NOT
orchestrate quiescence; it delivers writes via `mailw` and awaits outcome futures.

Per-request `quiescence_timeout_ms` and `quiet_window` hints SHALL be carried in
`DeliveryEnvelope` so the transport can apply per-write quiescence bounds. The
relay attaches these from the task's quiescence options at envelope construction
time and has no further involvement in scheduling the wait.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the transport flushes its write buffer and injects the pending messages

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** pane output continues changing
- **THEN** the transport keeps buffered writes pending
- **AND** flushes after a future quiescent window is observed

#### Scenario: Apply request quiescence timeout override

- **WHEN** a request provides `quiescence_timeout_ms`
- **AND** no quiescent window is observed before that timeout
- **THEN** the transport resolves the pending outcome futures with a timeout result
- **AND** records timeout in relay diagnostics/inscriptions

#### Scenario: Map request timeout to transport quiescence bound

- **WHEN** a request includes `quiescence_timeout_ms`
- **THEN** the relay attaches that value to the `DeliveryEnvelope` quiescence
  hints
- **AND** the transport uses it as the effective wait bound for those writes

#### Scenario: Quiescence hints from head envelope govern the flush group

- **WHEN** the Tmux transport accumulates multiple envelopes with differing
  quiescence hints into one flush group
- **THEN** it uses the `quiet_window` and `quiescence_timeout` from the first
  (head) envelope of the group as the effective bounds for the entire group
- **AND** a later envelope's timeout does not extend or shorten a wait already
  in progress for the group
