## ADDED Requirements

### Requirement: Tmux Prime Timeout

The system SHALL surface a config-surfaced, opt-in prime timeout knob for
Tmux-backed sessions, applied as the `tmux-prime-timeout-ms` TOML key,
applicable per-bundle and per-session. The knob SHALL bound the time
the Tmux transport waits, during the quiescence wait for a flush group,
for the target to produce observable output before classifying the flush
group as `unresponsive`. `None` (the default) preserves today's unbounded
behavior.

The prime timer SHALL start at the moment the Tmux transport's internal
delivery task begins the quiescence wait for a flush group. The prime
timer SHALL NOT reset on coalesce-during-wait when new envelopes are
absorbed into the flush group during the prime window.

The prime timer SHALL NOT classify a flush group as `unresponsive` while
`operator_interaction_active` reports an active copy-mode or key-table
for the target session. Active operator interaction indefinitely
suppresses unresponsive classification until it clears; the prime timer
SHALL NOT fire while operator interaction is active regardless of how
long the interaction persists.

When the prime timer fires (no observable output AND no operator
interaction within the prime window), the Tmux transport SHALL resolve
every sender in the flush group with `SendOutcome::Timeout`. The relay
worker SHALL propagate that outcome to the MCP/CLI caller as a distinct
timeout result, not collapsed into `Failed`.

#### Scenario: Prime timeout fires on unresponsive target

- **WHEN** the bundle/session config sets `tmux-prime-timeout-ms` to a
  finite value
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** the target pane produces no observable output before the prime
  window elapses
- **AND** `operator_interaction_active` is `None` for the target session
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout`
- **AND** no message is injected into the pane

#### Scenario: Prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle/session config does not set
  `tmux-prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not classify any flush group as
  `unresponsive`
- **AND** the only terminal failure modes for a flush group are
  `Failed` + `reason_code = "pane_wedged"` (if wedge detection is
  enabled and fires) and `Shutdown`

#### Scenario: Prime timeout does not fire while operator interaction is active

- **WHEN** the bundle/session config sets `tmux-prime-timeout-ms` to a
  finite value
- **AND** the prime window elapses with no observable output from the
  target
- **AND** `operator_interaction_active` reports an active copy-mode or
  key-table for the target session
- **THEN** the Tmux transport continues to wait
- **AND** does NOT classify the flush group as `unresponsive`
- **AND** the prime timer does NOT reset and does NOT fire while
  operator interaction remains active

#### Scenario: Prime timer does not reset on coalesce-during-wait

- **WHEN** the Tmux transport's internal delivery task is mid-prime-window
  for a flush group
- **AND** a new envelope arrives and is absorbed into the flush group
  via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the prime
  window

### Requirement: Tmux Wedged State Detection

The system SHALL surface a config-surfaced, opt-in wedge detection knob
for Tmux-backed sessions, applied as the `tmux-wedge-detection` TOML
key, applicable per-bundle and per-session. The knob SHALL classify a
settled, non-prompt-ready pane with no pending operator interaction as
`wedged`. `None` (the default) preserves today's unbounded behavior.

A wedge detection SHALL fire when the Tmux transport observes, during
the quiescence wait for a flush group:

- the pane output has been quiescent for at least one quiet window
- the prompt-readiness template does NOT match the inspected pane tail
- `operator_interaction_active` reports no active operator interaction

When wedge detection fires, the Tmux transport SHALL resolve every
sender in the flush group with `SendOutcome::Failed` and
`reason_code = "pane_wedged"`. The classification SHALL be sticky:
once the flush group is classified as wedged, the transport SHALL NOT
re-evaluate across coalesce iterations. Per-message wedge deadlines
within a flush group are out of scope.

#### Scenario: Wedge fires on settled non-prompt-ready pane

- **WHEN** the bundle/session config sets `tmux-wedge-detection` to a
  truthy value
- **AND** the Tmux transport's quiescence wait observes the pane
  becomes quiescent
- **AND** the prompt-readiness template does not match the inspected
  pane tail
- **AND** `operator_interaction_active` is `None`
- **THEN** every sender in the flush group receives
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** no message is injected into the pane

#### Scenario: Wedge detection defaults preserve unbounded behavior

- **WHEN** the bundle/session config does not set
  `tmux-wedge-detection` (or sets it to a falsy / `None` value)
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`

#### Scenario: Wedge does not fire while operator interaction is active

- **WHEN** the bundle/session config sets `tmux-wedge-detection` to a
  truthy value
- **AND** the Tmux transport's quiescence wait observes the pane
  becomes quiescent
- **AND** the prompt-readiness template does not match the inspected
  pane tail
- **AND** `operator_interaction_active` reports an active copy-mode or
  key-table for the target session
- **THEN** the transport continues to wait
- **AND** does NOT classify the flush group as `wedged`

#### Scenario: Wedge is sticky across coalesce iterations

- **WHEN** the Tmux transport's quiescence wait classifies a flush
  group as `wedged`
- **AND** new envelopes are absorbed into the flush group via
  coalesce-during-wait before the wedge classification propagates
- **THEN** every sender in the enlarged flush group receives the same
  wedge outcome (`Failed` + `reason_code = "pane_wedged"`)
- **AND** the transport does NOT re-evaluate wedge state across
  coalesce iterations

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

The semantics of `quiescence_timeout_ms` are transport-specific. On the
Tmux transport it bounds the prime window (no observable output before
the timeout); it SHALL NOT bound the post-quiescence prompt-readiness
wait, which is covered by the wedge detection and prompt-readiness
template requirements. On other transports the field MAY continue to
bound the quiescence wait per its existing semantics; transport-specific
behavior is recorded in the relevant transport spec.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the transport flushes its write buffer and injects the pending messages

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** pane output continues changing
- **THEN** the transport keeps buffered writes pending
- **AND** flushes after a future quiescent window is observed

#### Scenario: Apply request prime timeout override on Tmux

- **WHEN** a Tmux-bound request provides `quiescence_timeout_ms`
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** no observable output is produced before that timeout
- **AND** `operator_interaction_active` is `None` for the target session
- **THEN** the Tmux transport resolves the pending outcome futures with
  `SendOutcome::Timeout`
- **AND** records a `quiescence_timeout` inscription in relay diagnostics

#### Scenario: Tmux prime timeout does not bound post-quiescence wait

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **THEN** the Tmux transport SHALL NOT classify the flush group as
  `Timeout` solely on the basis of `quiescence_timeout_ms` elapsing
- **AND** the transport SHALL classify the flush group as `Failed` with
  `reason_code = "pane_wedged"` only when wedge detection is enabled and
  fires per the Tmux wedge detection requirement

#### Scenario: Map request timeout to transport quiescence bound

- **WHEN** a request includes `quiescence_timeout_ms`
- **THEN** the relay attaches that value to the `DeliveryEnvelope` quiescence
  hints
- **AND** the transport uses it as the effective prime-window bound on
  the Tmux transport

#### Scenario: Quiescence hints from head envelope govern the flush group

- **WHEN** the Tmux transport accumulates multiple envelopes with differing
  quiescence hints into one flush group
- **THEN** it uses the `quiet_window` and `quiescence_timeout` from the first
  (head) envelope of the group as the effective bounds for the entire group
- **AND** a later envelope's timeout does not extend or shorten a wait already
  in progress for the group

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

When `tmux-wedge-detection` is enabled for the target session, a settled
pane that does not match the prompt-readiness template and reports no
operator-interaction signal SHALL be classified as `wedged` rather than
left waiting indefinitely. The wedge detection knob is independent of
the prompt-readiness template configuration.

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
- **AND** relay continues waiting until wedge detection fires (if
  enabled), prime timeout fires (if enabled), or relay shuts down

#### Scenario: Classify as wedged when settled pane is not prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** `tmux-wedge-detection` is enabled for the target session
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **AND** no operator-interaction signal is active
- **THEN** the Tmux transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message

#### Scenario: Classify as unresponsive when prime window elapses

- **WHEN** target member has a prompt-readiness template
- **AND** `tmux-prime-timeout-ms` is enabled for the target session
- **AND** pane output never begins flowing within the prime window
- **AND** no operator-interaction signal is active
- **THEN** the Tmux transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Continue waiting when both classifiers disabled

- **WHEN** target member has a prompt-readiness template
- **AND** neither `tmux-wedge-detection` nor `tmux-prime-timeout-ms` is
  enabled for the target session
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay continues waiting until the pane becomes prompt-ready
  or the relay shuts down