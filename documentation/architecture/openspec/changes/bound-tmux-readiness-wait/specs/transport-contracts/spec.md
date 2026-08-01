## REMOVED Requirements

### Requirement: Tmux Wedged State Detection

**Reason:** the classifier infers a terminal failure from the absence of change
in rendered pane content, and that inference is unsound. A settled non-prompt
frame is produced by a hung coder, by a permission dialog awaiting an operator
decision, by a compose box holding typed input, and by a coder thinking with no
terminal output. These are indistinguishable from `capture-pane` output, so the
classifier reports a failure whenever it meets the three benign cases.

The cost is asymmetric. Every false positive is a message that failed and should
have landed; the only cost of not classifying is latency before a genuinely hung
pane is reported, which the readiness bound now supplies. The benign cases are
also the common ones, so the classifier bought speed on a rare condition by
being wrong on frequent ones.

**Migration:** no `format-version` bump, no compatibility shim, and no runtime or
persisted-state migration. The readiness bound introduced by this change
terminates every Tmux wait, so removing the classifier does not reintroduce an
unbounded wait. Deliveries that previously resolved `Failed` with
`reason_code = "pane_wedged"` now resolve `Timeout` at the readiness bound with
a reason describing the observation, and deliveries that previously failed
against a dialog, a compose box, or a briefly-settled pane now succeed once the
target returns to its prompt.

One operator edit is required. The `[coders.<id>.tmux].wedge-detection` key is
deleted outright, so a `coders.toml` that still sets it fails load on existing
unknown-field validation. An operator using the key SHALL delete the line; no
value preserves the prior behavior, because the behavior is gone. Absence of a
shim is not absence of an edit.

The identically named `[coders.<id>.pty].wedge-detection` key is unaffected. Pty
retains its own wedge detection under `Pty Wedged State Detection` until
`agentmux:issues/relay/61` supplies a Pty readiness bound; removing it before
then would leave Pty with no terminal path at all.

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
column.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled on
content that is not its prompt. A **cursor mismatch** (`prompt_regex` matched but
the reported cursor is not at `input_idle_cursor_column`) means the prompt frame
is healthy and the operator has input pending. Both mean the same thing
operationally — do not inject yet. What a transport may conclude from them
differs by transport, and is stated per transport below rather than universally.

**On Tmux** the distinction SHALL be a **diagnostic** one rather than a predicate
for terminal failure, and neither cause SHALL be treated as evidence that the
target has failed. A target that is not prompt-ready is a target that is not
ready *now*; the reason it is not ready is not knowable from the inspected tail.
A permission dialog awaiting an operator, a compose box holding typed input, a
coder producing no terminal output while working, and a hung process all present
as a settled non-prompt frame. The distinction survives only as the reason
reported if the wait later expires.

**On Pty** a frame mismatch still resolves `SendOutcome::Failed` with
`reason_code = "pane_wedged"` (see `Pty Wedged State Detection` and the scenario
below). That inference has exactly the soundness problem described above. It is
retained as a named temporary exception, because it is Pty's only terminal path
until `agentmux:issues/relay/61` supplies a Pty readiness bound, and removing it
first would leave Pty unable to end a wait at all. It SHALL NOT be read as
establishing that a transport without a readiness bound may infer failure from
rendered content. A cursor mismatch is not a failure predicate on either
transport.

For Tmux, a pane that never becomes prompt-ready — for either cause — SHALL be
bounded by the flush group's readiness bound (see the `delivery-quiescence`
capability's `Quiescence-Gated Delivery` requirement). That bound is the
**unconditional termination guarantee** for the post-quiescence wait: it applies
whatever the pane shows and whether or not a prime timeout is configured, and no
signal defers it. It is not the only way a Tmux wait can end — an opted-in prime
timeout, relay shutdown, and a positively observed probe or transport failure
each remain terminal — but it is the only one guaranteed to arrive.

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
- **AND** on Tmux, relay continues waiting until the target becomes prompt-ready,
  the readiness bound elapses, an enabled prime timeout elapses, or relay shuts
  down
- **AND** no terminal *failure* is issued on account of the pending input

> A cursor mismatch is not a frame mismatch, so the narrowing that keeps an
> elapsed prime timeout from adjudicating a settled non-matching frame does not
> reach this case: a pane whose frame matches while the operator is mid-keystroke
> still resolves on an enabled prime timeout. Whether it should is a live
> question — the same "a target that answered is not a silent target" argument
> applies — but narrowing it would change Pty, which this change holds fixed.
> Carried to `agentmux:issues/relay/61` with the rest of the Pty bound work.

#### Scenario: Do not inject into a pane awaiting an operator decision

- **WHEN** a Tmux target is displaying a prompt that awaits an operator response,
  such as a tool-permission request
- **AND** the pane is quiescent and `prompt_regex` does not match
- **THEN** relay does not inject the message
- **AND** relay does not report a terminal failure on account of the settled
  non-prompt frame
- **AND** the message is delivered once the operator answers and the pane returns
  to its prompt, provided the readiness bound has not elapsed
- **BECAUSE** a pane blocked on a human decision is neither ready nor failed, and
  the inspected tail cannot distinguish it from one that is

#### Scenario: Time out when pane output never begins flowing

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].prime-timeout-ms` is set to a
  finite millisecond value
- **AND** pane output never begins flowing within the prime window
- **THEN** the transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Classify a Pty settled frame mismatch as wedged (default-on)

- **WHEN** target member has a prompt-readiness template
- **AND** the coder defines `[coders.pty]` with `wedge-detection` not disabled
  (it defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** `prompt_regex` does not match the inspected pane tail
- **THEN** the Pty transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message
- **BECAUSE** Pty has no readiness bound until `agentmux:issues/relay/61`, so
  this remains its only terminal path despite sharing the Tmux transport's
  soundness problem

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: Pty wedge detection opt-out preserves prior behavior

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.pty].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay continues waiting until the pane becomes
  prompt-ready, prime timeout fires (if enabled), or relay shuts
  down

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
- **AND** the terminal failure modes for a flush group are `Timeout` from the
  readiness bound and `Shutdown`

#### Scenario: Prime timer does not reset on coalesce-during-wait

- **WHEN** the Tmux transport's internal delivery task is
  mid-prime-window for a flush group
- **AND** a new envelope arrives and is absorbed into the flush group
  via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the prime
  window
