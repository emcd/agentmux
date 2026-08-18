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

When `input_idle_cursor_column` is configured, the transport SHALL report itself
unable to accept a handover unless the cursor is at that configured column.

For a Tmux observation, a successful `prompt_regex` match SHALL be followed by
the OpenCode compose-region predicate when the inspected tail contains the
OpenCode frame suffix. The suffix is an info row with optional leading
whitespace and a content-bearing `┃`, followed immediately by a separator line
whose trimmed content is `╹` plus only 20 or more `▀` characters, then a status
row that begins with whitespace and contains `ctrl+p commands`. When multiple
valid suffixes occur in the inspected tail, the bottommost suffix SHALL be
selected. The predicate SHALL inspect exactly the three rows immediately
preceding that selected info row. An input row is compose text when optional
leading whitespace is followed by `┃`, then 2 through 99 whitespace characters
after the bar, and then a non-whitespace character. Rows with 100 or more
whitespace characters before content are sidebar rows, not compose text. A
matching OpenCode frame SHALL report itself able to accept a handover only when
none of those three rows contains compose text and the configured cursor
condition, if any, also succeeds.

The OpenCode predicate SHALL be internal to Tmux readiness evaluation. It SHALL
NOT add a prompt-template field and SHALL NOT apply to a non-OpenCode frame. A
non-OpenCode frame, or a frame with OpenCode-looking tokens that are not in the
adjacent info/separator/status arrangement, SHALL use the ordinary prompt-regex
and cursor conditions without the compose-region predicate. The predicate is
Tmux-only, and that is a consequence of the boundary below rather than a
limitation to be lifted later: what a rendered input box means is a question
about one transport's own surface. Pty evaluates `prompt_regex` and the
configured cursor column without it.

**The template gates authorization, and it gates it for every transport that has
one.** A target that is not prompt-ready SHALL NOT have a batch authorized for
it. This is a change in what the template does: it previously gated injection on
Tmux but on Pty only decided what the sender was told, because Pty had already
written the bytes.

**The template SHALL be evaluated by the transport that owns the target, never by
the relay.** The relay learns the result only as the level it reads through
`is_ready_for_handover`, and MUST NOT interpret `prompt_regex`, inspect pane
output, compare a cursor column, or apply the OpenCode compose predicate itself.

This is a decoupling boundary, not an implementation preference. Readiness
*determination* is transport-specific by nature and does not generalise: a prompt
regex over a pane tail is meaningless for ACP, whose readiness is the completion
of an earlier turn arriving on the wire protocol with no snapshot to inspect, and
meaningless again for UI, which reports itself unconditionally ready — a
broadcast surface has no turn to complete and no pane to render, so subscriber
presence is checked at the broadcast attempt itself rather than through
readiness. A relay that evaluated the template would be a relay that knows what
a pane is, which the `transport-abstraction` capability's `Transport Module
Boundaries` requirement forbids.

Readiness *scheduling* — deciding which target to visit, in what order, and when
to authorize — remains relay-owned and is transport-agnostic. The two are
separate concerns and only the second belongs to the relay.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled on
content that is not its prompt. An **OpenCode compose mismatch** (`prompt_regex`
matched, the adjacent OpenCode frame suffix was present, and compose text was
found in one of the three input rows) means the operator has text sitting in the
input box; it is Tmux-only. A **cursor mismatch** (`prompt_regex` matched, the
compose predicate passed where it applied, but the reported cursor is not at
`input_idle_cursor_column`) means the prompt frame is healthy and the operator
has input pending.

**Wherever a cause applies, it means the same thing operationally — do not
authorize yet — and none SHALL be treated as evidence that the target has
failed.** A target that is not prompt-ready is a target that is not ready *now*;
the reason is not knowable from the inspected tail. A permission dialog awaiting
an operator, a compose box holding typed input, a coder producing no terminal
output while working, and a hung process all present as a settled non-prompt
frame. The compose mismatch is the one cause whose meaning *is* known — the
operator is composing — and it too is only a reason to wait.

The per-transport split that previously let Pty conclude failure from a frame
mismatch is removed. No transport infers a terminal outcome from the template,
and the distinction between the causes survives only as diagnostic
observability.

A target that never becomes prompt-ready **while its transport remains healthy**
SHALL leave its entry `Pending` indefinitely. No bound converts that wait into an
outcome, because how long a target stays busy is not evidence about the target.
The wait is reported through the `delivery-quiescence` capability's
undelivered-queue inscriptions.

This is bounded by health, not by time. A transport that reports itself
unreachable past the dwell threshold resolves its members per the
`transport-abstraction` capability's `Transport Health as a Separate Axis`
requirement — not because the wait grew long, but because sustained
unreachability is evidence that no wait will end it.

#### Scenario: Authorize when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay authorizes the batch and the transport injects the message

#### Scenario: The relay never evaluates the template itself

- **WHEN** a target member has a prompt-readiness template
- **THEN** the relay delivery subsystem does not compile `prompt_regex`, read
  pane output, compare a cursor column, or apply the OpenCode compose predicate
- **AND** it authorizes solely on the level the transport reports through
  `is_ready_for_handover`

#### Scenario: A transport with no pane has no template to evaluate

- **WHEN** the target's transport determines readiness from a wire protocol
  observation, or reports itself unconditionally ready with no pane to observe
  at all
- **THEN** it reports `is_ready_for_handover` accordingly
- **AND** the relay authorizes on the same level it reads for every other
  transport, with no transport-specific branch

#### Scenario: A frame mismatch is not a failure on any transport

- **WHEN** a target's output is quiescent and `prompt_regex` does not match
- **THEN** no terminal outcome is issued on that basis
- **AND** the entry remains `Pending`
- **AND** this holds identically for Tmux, Pty, and every other transport

#### Scenario: A cursor mismatch defers authorization

- **WHEN** `prompt_regex` matches but the reported cursor is not at
  `input_idle_cursor_column`
- **THEN** relay does not authorize a batch for that target
- **AND** issues no terminal outcome

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** the transport reports itself able to accept a handover
- **AND** relay authorizes a batch for that target

#### Scenario: Authorize an idle OpenCode frame

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** each of the three input rows before the info row is empty or a
  sidebar row
- **AND** the configured cursor condition, if any, succeeds
- **THEN** the transport reports itself able to accept a handover
- **AND** relay authorizes the batch and the message is injected

#### Scenario: Withhold readiness while an OpenCode input row contains text

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** one of the three input rows before the info row contains `┃`,
  2 through 99 whitespace characters, and non-whitespace content
- **THEN** the transport reports itself unable to accept a handover
- **AND** relay does not authorize a batch for that target
- **AND** readiness reports an OpenCode compose mismatch as diagnostics
- **AND** the entry remains `Pending` with no terminal outcome issued on that
  basis
- **AND** the message is delivered once the operator clears the input box

#### Scenario: Treat the OpenCode sidebar boundary as non-compose content

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** the prompt regex and adjacent OpenCode frame suffix match
- **AND** an input row has 100 or more whitespace characters before content
- **THEN** the row does not cause an OpenCode compose mismatch
- **AND** the transport reports itself able to accept a handover when the
  remaining readiness conditions pass

#### Scenario: Preserve ordinary matching for a non-OpenCode frame

- **WHEN** a non-OpenCode Tmux matcher succeeds on a compose-like block
- **AND** the block has no adjacent OpenCode frame suffix
- **THEN** the OpenCode compose predicate is not applied
- **AND** readiness is evaluated using that matcher's ordinary cursor condition

#### Scenario: Ignore non-adjacent OpenCode-looking tokens

- **WHEN** a Tmux pane contains an info row and a status row containing
  `ctrl+p commands`
- **AND** the separator between them is absent, malformed, or not immediately
  adjacent to both rows
- **THEN** the OpenCode compose predicate is not applied
- **AND** readiness is evaluated using the ordinary prompt-regex and cursor
  conditions

#### Scenario: Require idle input column before injection

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay authorizes the batch and the message is injected

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not authorize a batch for that target
- **AND** relay continues waiting until the target becomes prompt-ready or relay
  shuts down
- **AND** no terminal *failure* is issued on account of the pending input

#### Scenario: Do not inject into a pane awaiting an operator decision

- **WHEN** a target is displaying a prompt that awaits an operator response,
  such as a tool-permission request
- **AND** the pane is quiescent and `prompt_regex` does not match
- **THEN** relay does not authorize a batch for that target
- **AND** relay does not report a terminal failure on account of the settled
  non-prompt frame
- **AND** the message is delivered once the operator answers and the pane returns
  to its prompt, however long the operator takes
- **BECAUSE** a pane blocked on a human decision is neither ready nor failed, and
  the inspected tail cannot distinguish it from one that is

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: A never-ready target waits without resolving

- **WHEN** a target's prompt-readiness template never matches, for arbitrarily
  long, while its transport keeps reporting itself reachable
- **THEN** the entry remains `Pending` and no terminal outcome is issued for it
- **AND** the most recent observation is recorded as diagnostics only, and does
  not accumulate toward any verdict
