## MODIFIED Requirements

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before relay injection.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output. The "pane output" source is
transport-specific: Tmux reads from `capture-pane`; Pty reads from
`Formatter::format_alloc(Format::Plain)` via the `PtyOutputView` look path.

When `input_idle_cursor_column` is configured, relay SHALL treat the target as
prompt-ready only when the transport reports the cursor at that configured
column. For Tmux, this is `tmux display-message -p`; for Pty, this is
`Terminal::cursor_x()`.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled
on content that is not its prompt. A **cursor mismatch** (`prompt_regex` matched
but the reported cursor is not at `input_idle_cursor_column`) means the prompt
frame is healthy and the operator has input pending. The two SHALL NOT be
treated as one condition: only a frame mismatch is wedge-class.

Wedge detection defaults to enabled for both Tmux-backed and Pty-
backed sessions (the operator MAY opt out per coder via
`[coders.<id>.{tmux,pty}].wedge-detection = false`). When wedge
detection is enabled and the pane settles at a **frame mismatch**, the coder
transport SHALL classify the flush group as `wedged`. A pane settled at a
cursor mismatch SHALL NOT be classified as `wedged` at any tick count.

The wedge classifier is the same `Wedged` outcome for both Tmux
and Pty: `SendOutcome::Failed` + `reason_code = "pane_wedged"`
after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
evaluations, OR when the prime window has elapsed with a wedge-
class mismatch observed. Per-transport knobs and Pty-specific
wedge scenarios live under the `Pty Wedged State Detection`
requirement; per-transport knobs live under the cross-cutting
`Pty Prime Timeout` requirement.

For Tmux, a pane that never becomes prompt-ready — for either cause — SHALL be
bounded by the flush group's readiness bound (see the `delivery-quiescence`
capability's `Quiescence-Gated Delivery` requirement), so waiting for the target
is bounded independently of whether a wedge verdict is ever reached. This
requirement does not state a bound for Pty; see `agentmux:issues/relay/61`.

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft included a per-transport "Operator-interaction
> semantics differ between transports" subsection + three
> `operator_interaction_active`-conditional scenarios (Tmux
> silence, Pty always-false, Pty-doesn't-consult). All three
> are obsolete after the upstream copy-mode gate was retired
> (issues/relay/52). Pty wedge scenarios moved to the `Pty
> Wedged State Detection` ADDED requirement.

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
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay injects the message

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not inject the message
- **AND** relay does NOT classify the flush group as `wedged`, however long the
  state persists
- **AND** on Tmux, relay continues waiting until the target becomes prompt-ready,
  the readiness bound elapses, or relay shuts down
- **BECAUSE** a matching frame with a displaced cursor is an operator composing
  input; wedging it would report a healthy target as stuck and inject into text
  the operator is still writing

#### Scenario: Time out when quiescent pane never becomes prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].prime-timeout-ms` is set to a
  finite millisecond value
- **AND** pane output never begins flowing within the prime window
- **THEN** the transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Classify as wedged when a settled pane has a frame mismatch (default-on)

- **WHEN** target member has a prompt-readiness template
- **AND** the coder defines `[coders.<id>.tmux]` or
  `[coders.<id>.pty]` with `wedge-detection` not disabled (it
  defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** `prompt_regex` does not match the inspected pane tail
- **THEN** the coder transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: Wedge detection opt-out suppresses the verdict, not the Tmux bound

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay does not issue a wedge verdict
- **AND** on Tmux, relay continues waiting until the pane becomes prompt-ready,
  the prime timeout fires (if enabled), the readiness bound elapses, or relay
  shuts down

### Requirement: Tmux Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for
Tmux-backed sessions, applied as the `prime-timeout-ms` TOML key under
the per-coder `[coders.<id>.tmux]` table (no `tmux-` prefix; the table
itself namespaces the key). The knob SHALL bound the time the Tmux
transport waits, during the quiescence wait for a flush group, for the
target to produce observable output before classifying the flush
group as `unresponsive`. The knob is **opt-in**: when absent or
`None`, no prime-window verdict is issued. Its absence SHALL NOT be read as an
unbounded wait; the Tmux readiness bound applies regardless (see the
`delivery-quiescence` capability's `Quiescence-Gated Delivery` requirement).

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
The readiness bound SHALL share that anchor.

No transport-observable operator rendering state (tmux copy-mode or a
non-`root` client key-table) SHALL suppress the prime timer. A quiescence
wait SHALL always progress toward one of its terminal classifications; the
prime timer SHALL NOT be held off indefinitely on the basis of a rendering
signal the relay cannot bound.

When the prime timer fires (no observable output within the prime
window), the Tmux transport SHALL resolve every sender in the flush group
with `SendOutcome::Timeout`. The `Timeout` outcome SHALL remain a distinct
terminal outcome and SHALL NOT be collapsed into `Failed`. As a non-delivered
outcome it SHALL be surfaced to the sender through the Asynchronous
Terminal-Outcome Receipt and recorded per Async Delivery Observability; it SHALL
NOT be returned in the synchronous accept-time response.

Reporting `Timeout` as a non-delivered outcome is sound for Tmux because
injection into the pane follows the wait, so a fired timer provably precedes
delivery.

#### Scenario: Prime timeout fires on unresponsive target

- **WHEN** the bundle config sets `[coders.<id>.tmux].prime-timeout-ms`
  to a finite millisecond value
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** the target pane produces no observable output before the
  prime window elapses
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout`
- **AND** no message is injected into the pane

#### Scenario: Absent prime timeout suppresses the prime verdict, not the bound

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not classify any flush group as
  `unresponsive` on the basis of the prime window
- **AND** the readiness bound still applies to the flush group
- **AND** the terminal failure modes for a flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default), `Timeout` from the readiness bound, and
  `Shutdown`

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
classify a settled pane whose prompt frame is absent as `wedged`.

Wedge detection is an early exit, not the termination guarantee. The wait for a
flush group is bounded by that group's readiness bound (see the
`delivery-quiescence` capability's `Quiescence-Gated Delivery` requirement)
whether or not wedge detection is enabled. Wedge detection only allows a verdict
to be reached sooner, with a more specific reason, when the diagnosis is
unambiguous.

Wedge detection defaults to **enabled** (`true`) — the cost of a
silently-wedged pane (delivery queue growth, silent failure) is
higher than the cost of a false-positive wedge (operator restarts the
target, future deliveries proceed normally). Operators MAY opt out by
setting `[coders.<id>.tmux].wedge-detection = false`. The opt-out
suppresses the early verdict only; it SHALL NOT restore an unbounded wait, and
the flush group still resolves as `Timeout` when its readiness bound elapses.

A wedge detection SHALL fire when wedge detection is enabled and the
Tmux transport observes, during the quiescence wait for a flush
group:

- the pane output has been quiescent, and
- the prompt-readiness template's **frame** did not match the inspected pane
  tail, and
- the same wedge-class mismatch signature has been observed across
  `WEDGE_CONSECUTIVE_TICKS` consecutive quiescent evaluations.

Those evaluations SHALL continue to accrue while the pane is settled, without
requiring the pane to change. A wedge condition that stops advancing because its
target stopped producing output is not detectable by the classifier that exists
to detect stopped targets.

A readiness mismatch SHALL NOT be treated as a single undifferentiated
condition. A **frame mismatch** (the prompt-readiness template did not match)
indicates the target has settled on content that is not its prompt, and is
wedge-class. A **cursor mismatch** (the template matched but the cursor is away
from its configured idle column) indicates a healthy prompt frame holding
pending operator input, and SHALL NOT be classified as wedged at any tick count;
it remains subject to the readiness bound like any other non-ready state.
An **empty inspected tail** indicates a target with no observable content and is
unresponsive rather than wedged.

No operator-interaction signal SHALL suppress these classifications. The Tmux
transport's target queries are limited to pane identity, window activity, and
cursor column, and it detects neither copy-mode nor an active key-table.
Documentation asserting that such a signal defers a classification or a bound
describes a gate that no longer exists and SHALL be removed rather than relied
upon, since it reads as covering the pending-operator-input case governed above.

When wedge detection fires, the Tmux transport SHALL resolve every
sender in the flush group with `SendOutcome::Failed` and
`reason_code = "pane_wedged"`. The classification SHALL be sticky:
once the flush group is classified as wedged, the transport SHALL NOT
re-evaluate across coalesce iterations. Per-message wedge deadlines
within a flush group are out of scope.

#### Scenario: Wedge fires on a settled pane with an absent frame (default-on)

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **AND** the Tmux transport's quiescence wait observes the pane becomes
  quiescent
- **AND** the prompt-readiness template's frame does not match the inspected
  pane tail
- **AND** that signature repeats across `WEDGE_CONSECUTIVE_TICKS` consecutive
  quiescent evaluations
- **THEN** every sender in the flush group receives
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** no message is injected into the pane

#### Scenario: Wedge evaluations accrue without target output

- **WHEN** a pane holds a settled frame-absent state and produces no further
  output
- **THEN** further quiescent evaluations still occur
- **AND** the verdict is reached without requiring any further target change
- **BECAUSE** a target that has stopped producing output is precisely the case
  wedge detection exists to catch

#### Scenario: Pending operator input is never wedged

- **WHEN** the prompt-readiness template matches the inspected pane tail
- **AND** the cursor is away from its configured idle column
- **AND** the pane remains quiescent in that state across arbitrarily many
  evaluations
- **THEN** no wedge verdict is issued
- **AND** the flush group remains pending until the target becomes ready or the
  readiness bound elapses
- **BECAUSE** a healthy prompt frame with a displaced cursor is an operator
  composing input, and injecting into it would corrupt what they are typing

#### Scenario: Wedge detection opt-out suppresses the verdict, not the bound

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence without issuing
  a wedge verdict
- **AND** the readiness bound still bounds that wait
- **AND** the terminal failure modes for the flush group are `Timeout` and
  `Shutdown`

#### Scenario: Wedge is sticky across coalesce iterations

- **WHEN** the Tmux transport's quiescence wait classifies a flush
  group as `wedged`
- **AND** new envelopes are absorbed into the flush group via
  coalesce-during-wait before the wedge classification propagates
- **THEN** every sender in the enlarged flush group receives the same
  wedge outcome (`Failed` + `reason_code = "pane_wedged"`)
- **AND** the transport does NOT re-evaluate wedge state across
  coalesce iterations
