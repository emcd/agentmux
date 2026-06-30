## ADDED Requirements

### Requirement: Tmux Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for
Tmux-backed sessions, applied as the `prime-timeout-ms` TOML key under
the per-coder `[coders.<id>.tmux]` table (no `tmux-` prefix; the table
itself namespaces the key). The knob SHALL bound the time the Tmux
transport waits, during the quiescence wait for a flush group, for the
target to produce observable output before classifying the flush
group as `unresponsive`. The knob is **opt-in**: when absent or
`None`, the Tmux transport preserves today's unbounded behavior.

The prime timeout SHALL be communicated from the relay to the Tmux
transport through a generic `DeliveryEnvelope.prime_timeout_ms:
Option<u64>` field. The relay populates this field from
`[coders.<id>.tmux].prime-timeout-ms` at envelope construction time.
The field is generic across transports: the relay does not know which
transport will consume it; the ACP delivery-side timeout follow-up
will populate the same field for ACP sessions from a corresponding
`[coders.<id>.acp].prime-timeout-ms` key.

The prime timer SHALL start at the moment the Tmux transport's
internal delivery task begins the quiescence wait for a flush group.
The prime timer SHALL NOT reset on coalesce-during-wait when new
envelopes are absorbed into the flush group during the prime window.

The prime timer SHALL NOT classify a flush group as `unresponsive`
while `operator_interaction_active` reports an active copy-mode or
key-table for the target session. Active operator interaction
indefinitely suppresses unresponsive classification until it clears;
the prime timer SHALL NOT fire while operator interaction is active
regardless of how long the interaction persists.

When the prime timer fires (no observable output AND no operator
interaction within the prime window), the Tmux transport SHALL
resolve every sender in the flush group with `SendOutcome::Timeout`.
The relay worker SHALL propagate that outcome to the MCP/CLI caller
as a distinct timeout result, not collapsed into `Failed`.

#### Scenario: Prime timeout fires on unresponsive target

- **WHEN** the bundle config sets `[coders.<id>.tmux].prime-timeout-ms`
  to a finite millisecond value
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** the target pane produces no observable output before the
  prime window elapses
- **AND** `operator_interaction_active` is `None` for the target
  session
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout`
- **AND** no message is injected into the pane

#### Scenario: Prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not classify any flush group as
  `unresponsive`
- **AND** the only terminal failure modes for a flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default) and `Shutdown`

#### Scenario: Prime timeout does not fire while operator interaction is active

- **WHEN** the bundle config sets `[coders.<id>.tmux].prime-timeout-ms`
  to a finite millisecond value
- **AND** the prime window elapses with no observable output from the
  target
- **AND** `operator_interaction_active` reports an active copy-mode
  or key-table for the target session
- **THEN** the Tmux transport continues to wait
- **AND** does NOT classify the flush group as `unresponsive`
- **AND** the prime timer does NOT reset and does NOT fire while
  operator interaction remains active

#### Scenario: Prime timer does not reset on coalesce-during-wait

- **WHEN** the Tmux transport's internal delivery task is
  mid-prime-window for a flush group
- **AND** a new envelope arrives and is absorbed into the flush group
  via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the prime
  window

### Requirement: Tmux Wedged State Detection

The system SHALL surface a config-surfaced wedge detection knob for
Tmux-backed sessions, applied as the `wedge-detection` boolean TOML
key under the per-coder `[coders.<id>.tmux]` table. The knob SHALL
classify a settled, non-prompt-ready pane with no pending operator
interaction as `wedged`.

Wedge detection defaults to **enabled** (`true`) — the cost of a
silently-wedged pane (delivery queue growth, silent failure) is
higher than the cost of a false-positive wedge (operator restarts the
target, future deliveries proceed normally). Operators MAY opt out by
setting `[coders.<id>.tmux].wedge-detection = false`. The opt-out
preserves today's unbounded-wait behavior.

A wedge detection SHALL fire when wedge detection is enabled and the
Tmux transport observes, during the quiescence wait for a flush
group:

- the pane output has been quiescent for at least one quiet window
- the prompt-readiness template does NOT match the inspected pane tail
- `operator_interaction_active` reports no active operator interaction

When wedge detection fires, the Tmux transport SHALL resolve every
sender in the flush group with `SendOutcome::Failed` and
`reason_code = "pane_wedged"`. The classification SHALL be sticky:
once the flush group is classified as wedged, the transport SHALL NOT
re-evaluate across coalesce iterations. Per-message wedge deadlines
within a flush group are out of scope.

#### Scenario: Wedge fires on settled non-prompt-ready pane (default-on)

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **AND** the Tmux transport's quiescence wait observes the pane
  becomes quiescent
- **AND** the prompt-readiness template does not match the inspected
  pane tail
- **AND** `operator_interaction_active` is `None`
- **THEN** every sender in the flush group receives
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** no message is injected into the pane

#### Scenario: Wedge detection opt-out preserves unbounded behavior

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`

#### Scenario: Wedge does not fire while operator interaction is active

- **WHEN** the bundle config does not disable wedge detection
- **AND** the Tmux transport's quiescence wait observes the pane
  becomes quiescent
- **AND** the prompt-readiness template does not match the inspected
  pane tail
- **AND** `operator_interaction_active` reports an active copy-mode
  or key-table for the target session
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

The relay SHALL communicate per-write quiescence bounds to the transport via
two `DeliveryEnvelope` fields:

- `quiet_window: Duration` — the quiet period before the transport
  declares the target ready to receive a flush group. Shared across all
  transports that perform quiescence waits.
- `prime_timeout_ms: Option<u64>` — generic prime-timeout bound that any
  prime-wait transport MAY consume. The relay populates this field from
  per-coder config (e.g. `[coders.<id>.tmux].prime-timeout-ms` for Tmux
  sessions; the ACP delivery-side timeout follow-up will populate the
  same field from `[coders.<id>.acp].prime-timeout-ms` for ACP sessions).

The Tmux transport's prime timeout bounds the prime window (no
observable output during the quiescence wait before the timeout). The
Tmux transport SHALL NOT use the prime timeout to bound the
post-quiescence prompt-readiness wait, which is governed by the wedge
detection requirement and the prompt-readiness template requirement.
The per-transport bound semantics are recorded in the relevant
transport spec.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the transport flushes its write buffer and injects the pending messages

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** pane output continues changing
- **THEN** the transport keeps buffered writes pending
- **AND** flushes after a future quiescent window is observed

#### Scenario: Apply request prime timeout override on Tmux

- **WHEN** a Tmux-bound request carries a non-`None`
  `DeliveryEnvelope.prime_timeout_ms`
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** no observable output is produced before that timeout
- **AND** `operator_interaction_active` is `None` for the target
  session
- **THEN** the Tmux transport resolves the pending outcome futures
  with `SendOutcome::Timeout`
- **AND** records a `delivery_prime_timeout` inscription in relay
  diagnostics

#### Scenario: Tmux prime timeout does not bound post-quiescence wait (wedge enabled)

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **AND** wedge detection is enabled (the default for the coder)
- **THEN** the Tmux transport SHALL NOT classify the flush group as
  `Timeout` solely on the basis of `prime_timeout_ms` elapsing
- **AND** the transport SHALL classify the flush group as `Failed`
  with `reason_code = "pane_wedged"` when the wedge detection
  requirement fires (after `WEDGE_CONSECUTIVE_TICKS` identical
  wedge-class evaluations or when the prime window has elapsed with
  a wedge-class mismatch observed)

#### Scenario: Tmux prime timeout bounds post-quiescence wait when wedge is disabled

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **AND** wedge detection is disabled via
  `[coders.<id>.tmux].wedge-detection = false`
- **AND** `prime_timeout_ms` is set to a finite millisecond value
- **THEN** the Tmux transport SHALL classify the flush group as
  `Timeout` when `prime_timeout_ms` elapses
- **BECAUSE** an operator who has explicitly disabled wedge detection
  and opted in to a prime timeout has accepted the bounded-wait
  semantics — the prime window is the only bounded-wait knob in
  effect, and it covers every quiescent state (including wedge-class
  content)

#### Scenario: Map Tmux prime timeout to transport envelope field

- **WHEN** a bundle member's `[coders.<id>.tmux].prime-timeout-ms` is
  set to a finite millisecond value
- **THEN** the relay attaches that value to the
  `DeliveryEnvelope.prime_timeout_ms` field at envelope construction
  time
- **AND** the Tmux transport uses it as the effective prime-window
  bound for the flush group

#### Scenario: Quiescence hints from head envelope govern the flush group

- **WHEN** the Tmux transport accumulates multiple envelopes with
  differing `quiet_window` or `prime_timeout_ms` values into one
  flush group
- **THEN** it uses the `quiet_window` and `prime_timeout_ms` from
  the first (head) envelope of the group as the effective bounds for
  the entire group
- **AND** a later envelope's prime timeout does not extend or
  shorten a wait already in progress for the group

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

Wedge detection defaults to enabled for all Tmux-backed sessions (the
operator MAY opt out per coder via
`[coders.<id>.tmux].wedge-detection = false`). When wedge detection is
enabled and the pane settles at a non-prompt-ready state with no
operator-interaction signal, the Tmux transport SHALL classify the
flush group as `wedged` rather than waiting indefinitely. The wedge
detection knob is independent of the prompt-readiness template
configuration.

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
- **AND** relay continues waiting until wedge detection fires (when
  enabled), prime timeout fires (when enabled), or relay shuts down

#### Scenario: Classify as wedged when settled pane is not prompt-ready (default-on)

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.tmux].wedge-detection` is not disabled (it
  defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **AND** no operator-interaction signal is active
- **THEN** the Tmux transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message

#### Scenario: Classify as unresponsive when prime window elapses

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.tmux].prime-timeout-ms` is set to a finite
  millisecond value
- **AND** pane output never begins flowing within the prime window
- **AND** no operator-interaction signal is active
- **THEN** the Tmux transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Wedge detection opt-out preserves prior behavior

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.tmux].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay continues waiting until the pane becomes
  prompt-ready, prime timeout fires (if enabled), or relay shuts
  down