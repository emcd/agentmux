## ADDED Requirements

### Requirement: ACP Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for
ACP-backed sessions, applied as the `prime-timeout-ms` TOML key
under the per-coder `[coders.<id>.acp]` table. The key name is
identical to the Tmux-side key `[coders.<id>.tmux].prime-timeout-ms`
so operator vocabulary is symmetric across transports; the table
itself namespaces the transport.

The knob SHALL bound the time the ACP transport's internal delivery
task waits, during the per-turn prompt completion wait for a flush
group, for the target to produce a terminal ACP response before
classifying the flush group as `unresponsive`. The knob is
**opt-in**: when absent or `None`, the ACP transport preserves
today's unbounded behavior.

The prime timeout SHALL be communicated from the relay to the ACP
transport through the generic
`DeliveryEnvelope.prime_timeout_ms: Option<u64>` field introduced by
the `tmux-wedge-detection` proposal. The relay populates this field
from `[coders.<id>.acp].prime-timeout-ms` at envelope construction
time, for ACP-backed sessions. The ACP transport consumes the field
to bound the per-turn wait; it does NOT introduce a
transport-prefixed envelope field on top of the generic one.

The prime timer SHALL start at the moment the ACP transport's
internal delivery task first enters the per-turn wait
(`wait_for_prompt_complete`). The prime timer SHALL NOT reset on
coalesce-during-wait when new envelopes are absorbed into the
flush group; absorbed envelopes inherit the head envelope's prime
timer anchor.

The prime timer SHALL NOT classify a flush group as `unresponsive`
while a `pending_choice_outcome` is in flight (an operator
decision is pending). The prime timer continues to wait without
firing until the choice resolves or the turn completes. This
matches the non-expiring choice pending lifecycle contract.

When the prime timer fires (no terminal `PromptCompletion` AND no
pending choice within the prime window), the ACP transport SHALL
resolve every sender in the flush group with `SendOutcome::Timeout`
and `reason_code = "acp_turn_timeout"`. The transport SHALL NOT
inject further messages into the wedge; the failure is terminal and
the relay records a `delivery_prime_timeout` inscription event
carrying `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`.
The per-target readiness SHALL be latched to `Unavailable` so the
worker's respawn-needed signal can re-bootstrap the runtime on the
same path used for `PromptCompletion::ConnectionClosed`.

The `acp_turn_timeout` reason code SHALL be reused; no new
`SendOutcome` variant is introduced. The mapping is consistent
with the `ACP Stop-Reason Outcome Mapping` requirement (which
defines `acp_turn_timeout` as the canonical ACP timeout reason
code).

The prime timeout SHALL be config-only in v1. The pre-existing
per-call override surfaces (`--acp-turn-timeout-ms` CLI flag and
`acp_turn_timeout_ms` MCP payload field) are RETIRED — `send`
carries no per-call timeout override field in v1 on either
transport. Operators configure the deadline via the per-coder
config key only. The retirement is symmetric with the
`tmux-wedge-detection` retirement of `--quiescence-timeout-ms` and
`quiescence_timeout_ms` for Tmux: v1 of both transports is fully
config-only.

#### Scenario: ACP prime timeout fires on unresponsive ACP target

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **AND** the ACP transport's internal delivery task first enters
  the per-turn prompt completion wait for a flush group
- **AND** the ACP target produces no terminal `PromptCompletion`
  before the prime window elapses
- **AND** no `pending_choice_outcome` is in flight
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout` with `reason_code = "acp_turn_timeout"`
- **AND** no further message is injected into the target's
  prompt
- **AND** a `delivery_prime_timeout` inscription is emitted with
  `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`

#### Scenario: ACP prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.acp].prime-timeout-ms` (or sets it to `None`)
- **THEN** the ACP transport does not classify any flush group
  as `unresponsive`
- **AND** the only terminal failure modes for a flush group are
  the existing `ACP Stop-Reason Outcome Mapping` outcomes and
  `DroppedOnShutdown`

#### Scenario: ACP prime timeout does not fire during pending choice

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **AND** the ACP target's agent raises a tool-call permission
  request mid-turn (the `pending_choice_outcome` slot is in
  flight)
- **AND** the prime window elapses without a terminal
  `PromptCompletion`
- **THEN** the ACP transport continues to wait
- **AND** does NOT classify the flush group as `unresponsive`
- **AND** the prime timer continues to count down without firing
  while the choice is pending
- **AND** once the choice resolves (`ChoiceMade::Chosen` or
  `ChoiceMade::Cancelled`), the prime timer resumes counting
  against the original anchor

#### Scenario: ACP prime timer does not reset on coalesce-during-wait

- **WHEN** the ACP transport's internal delivery task is
  mid-turn for a flush group
- **AND** a new envelope arrives and is absorbed into the flush
  group via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the
  prime window

#### Scenario: ACP prime timeout uses the generic envelope field

- **WHEN** an ACP-backed session has
  `[coders.<id>.acp].prime-timeout-ms` set to a finite
  millisecond value
- **THEN** the relay populates
  `DeliveryEnvelope.prime_timeout_ms` with that value at
  envelope construction time
- **AND** the ACP transport reads `prime_timeout_ms` to bound
  the prime wait
- **AND** no transport-prefixed envelope field (e.g.
  `acp_prime_timeout_ms`) is introduced

#### Scenario: ACP prime timeout uses the renamed operator knob

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **THEN** the `AcpTargetConfiguration.prime_timeout_ms` field
  (renamed from `turn_timeout_ms`) is validated at configuration
  load
- **AND** the prime timeout becomes load-bearing for the target
- **AND** operators who had configured the legacy
  `turn-timeout-ms` key (a key that does not exist in v1) see a
  `deny_unknown_fields` error from the raw loader on next bundle
  load

## MODIFIED Requirements

### Requirement: Non-Expiring Choice Pending Lifecycle

Alpha choice requests SHALL be non-expiring while relay and worker
state remain healthy.

Pending requests SHALL remain pending until one of:

- explicit authorized `selected` decision
- explicit authorized `cancelled` decision
- hard terminal cancellation condition (for example
  session/worker termination or aborted choice wait)

Relay SHALL NOT apply timer-based auto-expiry for choice requests
in alpha. The ACP prime timeout field
(`[coders.<id>.acp].prime-timeout-ms`) remains independent from
choice decision lifecycle.

#### Scenario: Keep choice request pending without timer expiry

- **WHEN** choice request is queued and no decision is made
- **AND** relay/worker remain healthy
- **THEN** request remains pending and is not auto-expired by
  timer