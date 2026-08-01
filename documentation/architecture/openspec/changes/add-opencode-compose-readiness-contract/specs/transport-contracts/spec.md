## MODIFIED Requirements

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates.

For Tmux, template matching SHALL gate injection into the pane. For Pty,
template matching SHALL gate only post-commit readiness and outcome resolution:
the transport writes every envelope to the PTY master before it begins the
readiness wait, so a later mismatch, timeout, or wedge outcome SHALL NOT claim
that the envelope bytes were not written.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output. The "pane output" source is
transport-specific: Tmux reads from `capture-pane`; Pty reads from
`Formatter::format_alloc(Format::Plain)` via the `PtyOutputView` look path.

When `input_idle_cursor_column` is configured, Tmux SHALL treat the target as
prompt-ready for injection only when the transport reports the cursor at that
configured column. Pty SHALL use the same condition only for its post-commit
readiness and outcome resolution.

For a Tmux observation, a successful `prompt_regex` match SHALL be followed by
the OpenCode compose-region predicate when the inspected tail contains the
OpenCode frame suffix. The suffix is an info row with optional leading
whitespace and a content-bearing `┃`, followed immediately by a separator
line whose trimmed content is `╹` plus only 20 or more `▀` characters, then a
status row that begins with whitespace and contains `ctrl+p commands`. When
multiple valid suffixes occur in the inspected tail, the bottommost suffix
SHALL be selected. The predicate SHALL inspect exactly the three rows
immediately preceding that selected info row. An input row is compose text
when optional leading whitespace is followed by `┃`, then 2 through 99
whitespace characters after the bar, and then a non-whitespace character. Rows
with 100 or more whitespace characters before content are sidebar rows, not
compose text. A matching OpenCode frame is prompt-ready only when none of
those three rows contains compose text and the configured cursor condition,
if any, also succeeds.

The OpenCode predicate SHALL be internal to Tmux readiness evaluation. It SHALL
not add a prompt-template field or apply to a non-OpenCode frame. A non-OpenCode
frame, or a frame with OpenCode-looking tokens that are not in the adjacent
info/separator/status arrangement, SHALL use the ordinary prompt-regex and
cursor conditions without the compose-region predicate.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled
on content that is not its prompt. An **OpenCode compose mismatch** means
`prompt_regex` matched, the adjacent OpenCode frame suffix was present, and
compose text was found in one of the three input rows. A **cursor mismatch**
(`prompt_regex` matched and the compose predicate, when applicable, passed,
but the reported cursor is not at `input_idle_cursor_column`) means the prompt
frame is healthy and the operator has input pending. On Tmux, a frame,
OpenCode compose, or cursor mismatch withholds injection. On Pty, an applicable
frame or cursor mismatch withholds only the post-commit Delivered/readiness
resolution; the envelope bytes have already been written. The OpenCode compose
mismatch is Tmux-only. What a transport may conclude from these mismatches is
therefore transport-specific.

**On Tmux** the distinction SHALL be a **diagnostic** one rather than a
predicate for terminal failure, and none of these causes SHALL be treated as
evidence that the target has failed. A target that is not prompt-ready is a
target that is not ready *now*; the reason it is not ready is not knowable from
the inspected tail. A permission dialog awaiting an operator, a compose box
holding typed input, a coder producing no terminal output while working, and a
hung process all present as a settled non-prompt frame. The distinction
survives only as the reason reported if the wait later expires.

**On Pty** a frame mismatch still resolves `SendOutcome::Failed` with
`reason_code = "pane_wedged"` (see `Pty Wedged State Detection` and the scenario
below), but the envelope bytes have already been written to the PTY master
before the readiness wait. That inference has exactly the soundness problem
described above. It is retained as a named temporary exception, because it is
Pty's only default-on terminal path until `agentmux:issues/relay/61` supplies a
Pty readiness bound, and removing it first would leave Pty unable to end a wait
at all. An opted-in Pty prime timeout and relay shutdown remain other terminal
paths. It SHALL NOT be read as establishing that a transport without a readiness
bound may infer failure from rendered content. A cursor mismatch is not a
failure predicate on either transport.

For Tmux, a pane that never becomes prompt-ready — for any frame, compose, or
cursor mismatch — SHALL be bounded by the flush group's readiness bound (see
the `delivery-quiescence` capability's `Quiescence-Gated Delivery`
requirement). That bound is the **unconditional termination guarantee** for the
post-quiescence wait: it applies whatever the pane shows and whether or not a
prime timeout is configured, and no signal defers it. It is not the only way a
Tmux wait can end — an opted-in prime timeout, relay shutdown, and a positively
observed probe or transport failure each remain terminal — but it is the only
one guaranteed to arrive.

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft included a per-transport "Operator-interaction
> semantics differ between transports" subsection + three
> `operator_interaction_active`-conditional scenarios (Tmux
> silence, Pty always-false, Pty-doesn't-consult). All three
> are obsolete after the upstream copy-mode gate was retired
> (issues/relay/52). Pty wedge scenarios moved to the
> `Pty Wedged State Detection` ADDED requirement.

#### Scenario: Tmux delivers when template matching conditions pass

- **WHEN** a Tmux target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **AND** either no OpenCode frame suffix is present or the OpenCode
  compose-region predicate finds no compose text
- **AND** the configured cursor condition, if any, succeeds
- **THEN** relay injects the message

#### Scenario: Pty resolves matching readiness after commit

- **WHEN** a Pty target member has a prompt-readiness template
- **AND** the envelope bytes have already been written to the PTY master
- **AND** `prompt_regex` matches the inspected multiline tail text
- **AND** the configured cursor condition, if any, succeeds
- **THEN** Pty resolves the post-commit readiness outcome according to the
  matching observation
- **AND** the outcome does not claim that the envelope bytes were unwritten

#### Scenario: Match prompt plus status with one multiline regex on Tmux

- **WHEN** a Tmux target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **AND** no OpenCode compose mismatch is present
- **THEN** relay treats target as prompt-ready

#### Scenario: Deliver to an idle OpenCode frame

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** the prompt regex matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** each of the three input rows before the info row is empty or a
  sidebar row
- **AND** the configured cursor condition, if any, succeeds
- **THEN** relay injects the message

#### Scenario: Do not inject while an OpenCode input row contains text

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** the prompt regex matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** one of the three input rows before the info row contains `┃`,
  2 through 99 whitespace characters, and non-whitespace content
- **THEN** relay does not inject the message
- **AND** readiness reports an OpenCode compose mismatch
- **AND** Tmux continues waiting until the target becomes prompt-ready, the
  readiness bound elapses, an enabled prime timeout elapses, or relay shuts
  down
- **AND** no terminal failure is issued on account of the compose mismatch

#### Scenario: Treat the OpenCode sidebar boundary as non-compose content

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** the prompt regex and adjacent OpenCode frame suffix match
- **AND** an input row has 100 or more whitespace characters before content
- **THEN** the row does not cause an OpenCode compose mismatch
- **AND** relay may inject when the remaining readiness conditions pass

#### Scenario: Preserve ordinary matching for a non-OpenCode frame

- **WHEN** a non-OpenCode Tmux matcher succeeds on a compose-like block
- **AND** the block has no adjacent OpenCode frame suffix
- **THEN** the OpenCode compose predicate is not applied
- **AND** relay evaluates readiness using that matcher's ordinary cursor
  condition

#### Scenario: Ignore non-adjacent OpenCode-looking tokens

- **WHEN** a Tmux pane contains an info row and a status row containing
  `ctrl+p commands`
- **AND** the separator between them is absent, malformed, or not immediately
  adjacent to both rows
- **THEN** the OpenCode compose predicate is not applied
- **AND** relay evaluates readiness using the ordinary prompt-regex and cursor
  conditions

#### Scenario: Require idle input column before Tmux injection

- **WHEN** a Tmux target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** any applicable OpenCode compose predicate passes
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay injects the message

#### Scenario: Do not inject into Tmux while user is typing

- **WHEN** a Tmux target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** any applicable OpenCode compose predicate passes
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not inject the message
- **AND** on Tmux, relay continues waiting until the target becomes prompt-ready,
  the readiness bound elapses, an enabled prime timeout elapses, or relay shuts
  down
- **AND** no terminal *failure* is issued on account of the pending input

> A cursor mismatch is not a frame or compose mismatch, so the narrowing that
> keeps an elapsed prime timeout from adjudicating a settled non-matching frame
> does not reach this case: a pane whose frame matches while the operator is
> mid-keystroke still resolves on an enabled prime timeout. Whether it should is
> a live question — the same "a target that answered is not a silent target"
> argument applies — but narrowing it would change Pty, which this change holds
> fixed. Carried to `agentmux:issues/relay/61` with the rest of the Pty bound
> work.

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

#### Scenario: Resolve a non-ready wait after the transport commitment

- **WHEN** a target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].prime-timeout-ms` is set to a
  finite millisecond value
- **AND** pane output never begins flowing within the prime window
- **THEN** the transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** for Tmux, relay does not inject the message
- **AND** for Pty, the envelope bytes were already written before the wait

#### Scenario: Classify a post-commit Pty settled frame mismatch as wedged (default-on)

- **WHEN** a Pty target member has a prompt-readiness template
- **AND** the coder defines `[coders.pty]` with `wedge-detection` not disabled
  (it defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** `prompt_regex` does not match the inspected pane tail
- **AND** the envelope bytes have already been written to the PTY master
- **THEN** the Pty transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** the outcome does not claim that the envelope bytes were unwritten
- **BECAUSE** Pty has no readiness bound until `agentmux:issues/relay/61`, so
  this remains its only default-on terminal path despite sharing the Tmux transport's
  soundness problem

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: Pty wedge detection opt-out preserves post-commit waiting

- **WHEN** a Pty target member has a prompt-readiness template
- **AND** `[coders.<id>.pty].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **AND** the envelope bytes have already been written to the PTY master
- **THEN** relay continues waiting until the pane becomes
  prompt-ready, prime timeout fires (if enabled), or relay shuts
  down
