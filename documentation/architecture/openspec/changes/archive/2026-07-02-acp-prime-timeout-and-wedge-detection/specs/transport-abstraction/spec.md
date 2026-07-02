## ADDED Requirements

### Requirement: ACP Prime Timeout Envelope Field Consumption

The ACP transport SHALL consume the generic
`DeliveryEnvelope.prime_timeout_ms: Option<u64>` field on the
envelope it receives via `mailw` / `raww`. The transport SHALL treat
`None` as unbounded (preserving today's behavior); it SHALL treat
`Some(ms)` as the prime window bound for the per-turn wait. The ACP
transport SHALL NOT introduce a transport-prefixed envelope field on
top of the generic `prime_timeout_ms` field.

The prime timer anchor SHALL be "delivery task perspective" — the
moment the ACP transport's internal delivery task first enters the
per-turn `wait_for_prompt_complete` poll, NOT the moment the relay
enqueues the task. The prime timer SHALL NOT reset on
coalesce-during-wait; absorbed envelopes inherit the head envelope's
prime timer anchor.

The prime timer SHALL NOT fire while a `pending_choice_outcome` is
in flight (an operator decision is pending). The transport SHALL
continue to wait without firing the prime timer until the choice
resolves or the turn completes.

On prime-timer fire, the transport SHALL resolve the flush group
with `SendOutcome::Timeout` and `reason_code = "acp_turn_timeout"`,
latch the per-target readiness to `Unavailable`, and signal
respawn-needed through the same path used for
`PromptCompletion::ConnectionClosed`. The transport SHALL NOT inject
further messages into the wedge.

#### Scenario: ACP transport consumes the generic prime timeout field

- **WHEN** an ACP target receives a `DeliveryEnvelope` with
  `prime_timeout_ms = Some(ms)`
- **THEN** the ACP transport reads the field and uses it as the
  prime window bound for the per-turn wait
- **AND** the transport does NOT introduce a separate
  `acp_prime_timeout_ms` envelope field

#### Scenario: ACP transport ignores prime timeout when None

- **WHEN** an ACP target receives a `DeliveryEnvelope` with
  `prime_timeout_ms = None`
- **THEN** the ACP transport preserves today's unbounded behavior
- **AND** the only terminal failure modes are the existing
  `ACP Stop-Reason Outcome Mapping` outcomes and `DroppedOnShutdown`

#### Scenario: ACP transport resolves flush group on prime timer fire

- **WHEN** the ACP transport's prime timer fires for a flush group
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout` with `reason_code = "acp_turn_timeout"`
- **AND** the per-target readiness is latched to `Unavailable`
- **AND** the respawn-needed signal is raised so the worker's
  `check_respawn_needed()` returns `true`
- **AND** a `delivery_prime_timeout` inscription is emitted with
  `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`