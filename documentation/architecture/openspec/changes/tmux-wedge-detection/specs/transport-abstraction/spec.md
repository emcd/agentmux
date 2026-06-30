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

The `unresponsive` and `wedged` classifiers SHALL be config-surfaced and
opt-in per the per-transport spec (see `session-relay` Tmux Prime Timeout
and Tmux Wedged State Detection requirements for the Tmux surface). A
`None` value on either config key preserves unbounded behavior on that
classifier independently of the other. The classifiers SHALL NOT depend
on each other — operators MAY enable prime timeout without wedge
detection, and vice versa.

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
- **AND** the relay worker treats the resulting `SingleDeliveryOutcome` as
  terminal regardless of which classifier fired

#### Scenario: Wedge detection is opt-in via config

- **WHEN** the bundle/session config does not enable wedge detection for
  the target transport
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`
  (if relay shutdown is requested)

#### Scenario: Prime timeout is opt-in via config

- **WHEN** the bundle/session config does not enable prime timeout for
  the target transport
- **THEN** the Tmux transport does not fire `Timeout` for unresponsive
  targets regardless of how long output remains absent
- **AND** the only terminal failure modes for the flush group are
  `Failed` + `reason_code = "pane_wedged"` (if wedge detection is
  enabled and fires) and `Shutdown`

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
  - `AlwaysWedgeProbe` → `SendOutcome::Failed` + `reason_code = "pane_wedged"`
  - `PendingChoiceProbe` → neither timeout nor wedge; the transport
    continues to wait indefinitely while operator interaction is
    active and the prime timer does NOT fire
  - `SlowPromptProbe` → `Delivered` after several quiescence ticks
  - `NormalFlowProbe` → `Delivered` without prime or wedge firing