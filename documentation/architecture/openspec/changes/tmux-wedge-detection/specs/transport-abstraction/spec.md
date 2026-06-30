## ADDED Requirements

### Requirement: Three-State Delivery Classifier

Promptable transports that gate delivery on a quiescence wait SHALL classify
each pending flush group, during the quiescence wait for that group, into
one of three terminal states:

- `running` — output is flowing or has settled at the prompt-readiness
  match; the transport continues to wait normally and resolves the flush
  group as `Delivered` when the prompt becomes ready.
- `unresponsive` — during the quiescence wait for the flush group, no
  observable output has been produced within the prime window AND no
  operator-interaction signal is active; the transport resolves the flush
  group as `SendOutcome::Timeout`.
- `wedged` — during the quiescence wait for the flush group, output has
  settled, the prompt-readiness template does not match, and no
  operator-interaction signal is active; the transport resolves the flush
  group as `SendOutcome::Failed` with a transport-defined `reason_code`
  on the same `Failed` variant (for the Tmux transport,
  `reason_code = "pane_wedged"`).

The `unresponsive` and `wedged` classifiers SHALL each be config-surfaced
per the per-transport spec (see `session-relay` Tmux Prime Timeout and
Tmux Wedged State Detection requirements for the Tmux surface).

- The Tmux `unresponsive` classifier SHALL be **opt-in**: absent or
  `None` on `[coders.<id>.tmux].prime-timeout-ms` preserves today's
  unbounded behavior.
- The Tmux `wedged` classifier SHALL be **opt-out**: it defaults to
  enabled (`wedge-detection` is `true` when absent or `true`),
  because the cost of a silently-wedged pane is higher than the cost
  of a false-positive wedge. Operators MAY set
  `[coders.<id>.tmux].wedge-detection = false` to preserve the prior
  unbounded-wait behavior.

Active operator-interaction signals (such as tmux copy-mode or active
key-table for the Tmux transport) SHALL indefinitely suppress both
`unresponsive` and `wedged` classification while they remain active.
The classifier SHALL NOT fire any failure classification while
operator-interaction is active.

The classifier SHALL be evaluated at the transport's quiescence wait,
NOT at the relay delivery worker. The relay SHALL NOT inspect
`SingleDeliveryOutcome` to make delivery policy decisions; it only
relays the outcome to the MCP/CLI caller and to the diagnostic stream.

The three states are mutually exclusive at the moment of terminal
classification. The classifier SHALL NOT combine them (for example, a
flush group SHALL NOT resolve as `Timeout AND Failed`).

#### Scenario: Tmux delivery classifies into one of three states

- **WHEN** the Tmux transport's quiescence wait observes the target's
  output state during the wait for a flush group
- **THEN** it routes the flush group to exactly one of `Delivered`,
  `Timeout`, or `Failed` with `reason_code = "pane_wedged"`
- **AND** the relay worker treats the resulting `SingleDeliveryOutcome`
  as terminal regardless of which classifier fired

#### Scenario: Tmux wedge detection defaults to enabled

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **THEN** the Tmux transport classifies a settled, non-prompt-ready,
  no-operator-interaction pane as `wedged`
- **AND** resolves the flush group as `Failed` with
  `reason_code = "pane_wedged"`

#### Scenario: Tmux wedge detection opt-out preserves prior behavior

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`
  (if relay shutdown is requested)

#### Scenario: Tmux prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not fire `Timeout` for unresponsive
  targets regardless of how long output remains absent
- **AND** the only terminal failure modes for the flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default) and `Shutdown`

#### Scenario: Wedge classification requires no pending operator interaction

- **WHEN** the Tmux transport's quiescence wait observes a settled pane
  that does not match the prompt-readiness template
- **AND** `operator_interaction_active` reports an active copy-mode or
  key-table for the target session
- **THEN** the transport continues to wait and does NOT classify the
  flush group as `wedged`
- **AND** this suppression persists for as long as
  `operator_interaction_active` remains active

#### Scenario: Unresponsive classification requires no pending operator interaction

- **WHEN** the Tmux transport's prime window elapses during the
  quiescence wait for a flush group with no observable output from the
  target
- **AND** `operator_interaction_active` reports an active copy-mode or
  key-table for the target session
- **THEN** the transport continues to wait and does NOT classify the
  flush group as `unresponsive`
- **AND** the prime timer does NOT reset and does NOT fire while
  operator interaction remains active

#### Scenario: Group atomicity on failure classification

- **WHEN** the Tmux transport's quiescence wait classifies the flush
  group as `unresponsive` or `wedged`
- **THEN** every sender in the flush group receives the same terminal
  outcome
- **AND** the transport does NOT classify individual envelopes
  independently within the same flush group

### Requirement: Prime Timeout Envelope Field

The relay SHALL communicate a per-write prime-timeout bound to transports
via a generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field.
The field SHALL be transport-neutral — the relay populates it from
per-coder config without knowing which transport will consume it, and
each transport that performs a prime wait MAY read it or ignore it.

For Tmux-backed sessions, the relay populates
`DeliveryEnvelope.prime_timeout_ms` from
`[coders.<id>.tmux].prime-timeout-ms`. The ACP delivery-side timeout
follow-up will populate the same field for ACP sessions from
`[coders.<id>.acp].prime-timeout-ms` (or a parallel per-coder key under
the ACP table).

The field SHALL replace any prior transport-specific prime-timeout
field shape. The relay SHALL NOT add per-transport timeout fields to
`DeliveryEnvelope` — keeping the envelope transport-neutral preserves
the decoupling arc.

#### Scenario: Tmux prime timeout rides on the generic envelope field

- **WHEN** a Tmux-backed session has
  `[coders.<id>.tmux].prime-timeout-ms` set to a finite millisecond
  value
- **THEN** the relay populates `DeliveryEnvelope.prime_timeout_ms`
  with that value at envelope construction time
- **AND** the Tmux transport reads `prime_timeout_ms` to bound the
  prime window

#### Scenario: ACP follow-up consumes the same generic field

- **WHEN** the ACP delivery-side timeout follow-up lands
- **THEN** it populates `DeliveryEnvelope.prime_timeout_ms` from its
  own per-coder config key for ACP sessions
- **AND** does NOT introduce a transport-prefixed envelope field
  (e.g. `acp_prime_timeout_ms`)

#### Scenario: Transports ignore the field when not relevant

- **WHEN** a transport does not perform a prime wait (e.g. UI today)
- **THEN** it ignores `DeliveryEnvelope.prime_timeout_ms`
- **AND** the relay still populates the field with the configured
  value (the relay does not gate the population on transport type)

### Requirement: Transport-Internal Probe Seam for Testability

Each promptable transport that owns a quiescence wait SHALL expose an
internal probe trait that lets tests inject deterministic quiescence,
prompt-readiness, and operator-interaction results. The probe trait
SHALL be transport-internal (not part of the `Transport` contract) and
SHALL NOT appear in `src/transports/contract.rs`.

The probe trait SHALL return the next observation on demand so tests can
drive the classifier through specific sequences. The probe SHALL cover
at minimum the five canonical sequences: unresponsive, wedged,
pending-choice, slow-prompt, and normal-flow.

#### Scenario: Tmux probe trait is transport-internal

- **WHEN** a developer reads `src/tmux/transport.rs`
- **THEN** they find a `PaneQuiescenceProbe` trait used by
  `wait_for_quiescent_pane`
- **AND** the trait is not re-exported from `src/transports/`
- **AND** the `Transport` trait in `src/transports/contract.rs` has no
  knowledge of probes

#### Scenario: Tmux unit tests cover the five canonical sequences

- **WHEN** `cargo test --test tmux_transport` runs
- **THEN** it asserts the five canonical probe sequences produce the
  expected terminal outcomes:
  - `AlwaysUnresponsiveProbe` → `SendOutcome::Timeout`
  - `AlwaysWedgeProbe` → `SendOutcome::Failed` +
    `reason_code = "pane_wedged"`
  - `PendingChoiceProbe` → neither timeout nor wedge; the transport
    continues to wait indefinitely while operator interaction is
    active and the prime timer does NOT fire
  - `SlowPromptProbe` → `Delivered` after several quiescence ticks
  - `NormalFlowProbe` → `Delivered` without prime or wedge firing

#### Scenario: Tmux unit tests cover wedge default-on and opt-out

- **WHEN** `cargo test --test tmux_transport` runs
- **THEN** a test asserts the wedge classifier fires by default when
  `[coders.<id>.tmux].wedge-detection` is absent
- **AND** a test asserts the wedge classifier does NOT fire when
  `[coders.<id>.tmux].wedge-detection = false`