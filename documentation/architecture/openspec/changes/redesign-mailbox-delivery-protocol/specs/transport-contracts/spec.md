## MODIFIED Requirements

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before a transport writes a peeked mail entry.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output. The "pane output" source is
transport-specific: Tmux reads from `capture-pane`; Pty reads from
`Formatter::format_alloc(Format::Plain)` via the `PtyOutputView` look path.

When `input_idle_cursor_column` is configured, the transport SHALL treat
itself as not ready to write unless the cursor is at that configured column.

For a Tmux observation, a successful `prompt_regex` match SHALL be followed
by the OpenCode compose-region predicate when the inspected tail contains the
OpenCode frame suffix. The suffix is an info row with optional leading
whitespace and a content-bearing `┃`, followed immediately by a separator
line whose trimmed content is `╹` plus only 20 or more `▀` characters, then a
status row that begins with whitespace and contains `ctrl+p commands`. When
multiple valid suffixes occur in the inspected tail, the bottommost suffix
SHALL be selected. The predicate SHALL inspect exactly the three rows
immediately preceding that selected info row. An input row is compose text
when optional leading whitespace is followed by `┃`, then 2 through 99
whitespace characters after the bar, and then a non-whitespace character.
Rows with 100 or more whitespace characters before content are sidebar rows,
not compose text. A matching OpenCode frame SHALL be treated as ready to
write only when none of those three rows contains compose text and the
configured cursor condition, if any, also succeeds.

The OpenCode predicate SHALL be internal to Tmux readiness evaluation. It
SHALL NOT add a prompt-template field and SHALL NOT apply to a non-OpenCode
frame. A non-OpenCode frame, or a frame with OpenCode-looking tokens that are
not in the adjacent info/separator/status arrangement, SHALL use the
ordinary prompt-regex and cursor conditions without the compose-region
predicate. Pty evaluates `prompt_regex` and the configured cursor column
without it.

**The template gates a transport's own decision to write, for every
transport that has one. It gates nothing relay-facing.** A target that is
not prompt-ready SHALL NOT have a peeked entry written by its transport. This
is fully transport-internal: there is no relay authorization step left to
gate, and the relay reads no level from the transport at all under this
capability — `is_ready_for_handover` no longer exists. A transport that
decides not to write simply leaves the entry `queued`; it does not `ack`
it.

**The template SHALL be evaluated by the transport that owns the target,
never by the relay.** This was already true when the template gated
relay authorization; it remains true now that it gates the transport's own
write decision, and is if anything a tighter fit, since there is no longer
even a level for the relay to read.

Readiness *determination* is transport-specific by nature and does not
generalise: a prompt regex over a pane tail is meaningless for ACP, whose
readiness is the completion of an earlier turn arriving on the wire protocol
with no snapshot to inspect, and meaningless again for UI, which is
unconditionally ready to write — a broadcast surface has no turn to complete
and no pane to render.

A readiness failure SHALL be distinguished by its cause, purely as
diagnostic observability internal to the transport: a **frame mismatch**
(`prompt_regex` did not match), an **OpenCode compose mismatch**
(Tmux-only, compose text found in an input row), or a **cursor mismatch**
(the reported cursor is not at `input_idle_cursor_column`). Wherever a cause
applies, it means the same thing operationally — do not write yet — and none
SHALL be treated as evidence that the target has failed.

A target that never becomes prompt-ready **while its transport remains
healthy** SHALL leave its entries `queued` indefinitely. No bound converts
that wait into an outcome. The wait is reported through the
`delivery-quiescence` capability's undelivered-mailbox inscriptions.

This is bounded by health, not by time. A transport that reports itself
unreachable past the dwell threshold resolves its members per the
`transport-abstraction` capability's `Transport Health as a Separate Axis`
requirement — not because the wait grew long, but because sustained
unreachability is evidence that no wait will end it.

#### Scenario: Write when prompt-readiness template matches

- **WHEN** a peeked target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** the transport writes the peeked entry and acks it

#### Scenario: The relay never evaluates the template itself

- **WHEN** a target member has a prompt-readiness template
- **THEN** the relay delivery subsystem does not compile `prompt_regex`, read
  pane output, compare a cursor column, or apply the OpenCode compose
  predicate
- **AND** the relay reads no readiness level from the transport at all —
  readiness is not part of `peek` or `ack`

#### Scenario: A frame mismatch is not a failure on any transport

- **WHEN** a target's output is quiescent and `prompt_regex` does not match
- **THEN** no terminal outcome is issued on that basis
- **AND** the peeked entry remains `queued`, unacked
- **AND** this holds identically for Tmux, Pty, and every other transport

#### Scenario: A cursor mismatch defers writing

- **WHEN** `prompt_regex` matches but the reported cursor is not at
  `input_idle_cursor_column`
- **THEN** the transport does not write the peeked entry
- **AND** issues no terminal outcome, and does not ack it

#### Scenario: A transport with no pane has no template to evaluate

- **WHEN** the target's transport determines readiness from a wire protocol
  observation, or is unconditionally ready to write with no pane to observe
  at all
- **THEN** it writes according to that determination
- **AND** applies no prompt-regex, cursor, or OpenCode-compose evaluation

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** a target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** the transport writes the peeked entry

#### Scenario: Write to an idle OpenCode frame

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent and `prompt_regex` matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** each of the three input rows before the info row is empty or a
  sidebar row
- **AND** the configured cursor condition, if any, succeeds
- **THEN** the transport writes the peeked entry

#### Scenario: Treat the OpenCode sidebar boundary as non-compose content

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** the prompt regex and adjacent OpenCode frame suffix match
- **AND** an input row has 100 or more whitespace characters before content
- **THEN** the row does not cause an OpenCode compose mismatch
- **AND** the transport writes the peeked entry when the remaining readiness
  conditions pass

#### Scenario: Preserve ordinary matching for a non-OpenCode frame

- **WHEN** a non-OpenCode Tmux matcher succeeds on a compose-like block
- **AND** the block has no adjacent OpenCode frame suffix
- **THEN** the OpenCode compose predicate is not applied
- **AND** readiness is evaluated using that matcher's ordinary cursor
  condition

#### Scenario: Ignore non-adjacent OpenCode-looking tokens

- **WHEN** a Tmux pane contains an info row and a status row containing
  `ctrl+p commands`
- **AND** the separator between them is absent, malformed, or not
  immediately adjacent to both rows
- **THEN** the OpenCode compose predicate is not applied
- **AND** readiness is evaluated using the ordinary prompt-regex and cursor
  conditions

#### Scenario: Require idle input column before writing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent and `prompt_regex` matches
- **AND** the transport-reported cursor position equals the configured
  `input_idle_cursor_column`
- **THEN** the transport writes the peeked entry

#### Scenario: Do not write while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent and `prompt_regex` matches
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** the transport does not write the peeked entry
- **AND** it keeps re-peeking until the target becomes prompt-ready or the
  relay shuts down
- **AND** no terminal *failure* is issued on account of the pending input

#### Scenario: Do not write into a pane awaiting an operator decision

- **WHEN** a target is displaying a prompt that awaits an operator response,
  such as a tool-permission request
- **AND** the pane is quiescent and `prompt_regex` does not match
- **THEN** the transport does not write the peeked entry
- **AND** does not report a terminal failure on account of the settled
  non-prompt frame
- **AND** the message is written once the operator answers and the pane
  returns to its prompt, however long the operator takes
- **BECAUSE** a pane blocked on a human decision is neither ready nor
  failed, and the inspected tail cannot distinguish it from one that is

#### Scenario: Withhold writing while an OpenCode input row contains text

- **WHEN** a Tmux target has a prompt-readiness template
- **AND** pane output is quiescent and `prompt_regex` matches
- **AND** the adjacent OpenCode frame suffix is present
- **AND** one of the three input rows before the info row contains `┃`,
  2 through 99 whitespace characters, and non-whitespace content
- **THEN** the transport treats itself as not ready to write
- **AND** the peeked entry remains `queued` with no terminal outcome issued
- **AND** it is written once the operator clears the input box and the
  transport peeks again

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the operator
  scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** the transport writes the message
- **AND** the pane remains in copy-mode with the operator's scroll position
  undisturbed

#### Scenario: A never-ready target waits without resolving

- **WHEN** a target's prompt-readiness template never matches, for
  arbitrarily long, while its transport keeps reporting itself reachable
- **THEN** its entries remain `queued` and no terminal outcome is issued for
  them
- **AND** the most recent observation is recorded as diagnostics only, and
  does not accumulate toward any verdict

### Requirement: Relay raww transport behavior

A raw-kind mailbox entry SHALL be discovered by a transport's delivery-loop
executor through `peek`, exactly as specified by `delivery-quiescence`'s
`Mailbox Peek Operation` requirement: `peek` returns a raw entry only as a
singleton, never combined with mail. Before writing it, the executor SHALL
`declare` it — as its own singleton packing unit, per `Mailbox Submission
Declaration` — exactly as it would declare a mail packing unit; a raw entry
carries no exemption from the declare-before-write discipline.

Once peeked and declared, a transport's write of raw content SHALL map as
follows:

- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using the existing shared ACP worker/client path
  via `session/prompt`
- pty target: write `text` to the PTY master; if `no_enter=false`, write the
  terminating newline after it
- ui target: unsupported. `UiTransport`'s delivery-loop executor treats a raw
  entry it peeks as `Failed` with `reason_code = ui_raw_write_unsupported`
  and writes nothing

The transport SHALL treat raww `text` as opaque input and SHALL NOT evaluate
shell expansion or command substitution.

**Ordering.** Mail and raw are variants of one per-target mailbox. `peek`'s
own contract — a raw entry at the head is always returned alone, and mail
past an unpeeked raw entry is never returned — is what enforces the FIFO
barrier structurally: a transport's delivery-loop executor cannot see a raw
entry's successors until that raw entry itself has been acked, and cannot
see mail that precedes an unacked raw entry skipped over.

**Target-side ordering safety within one generation follows from the single
serial delivery executor, not from an additional wait.** Because one
transport instance runs exactly one serial delivery-loop executor for its
lifetime (`delivery-quiescence`'s `Consumer Generation Ownership and
Replacement`), that executor's own write calls are already sequential: it
cannot begin writing a raw entry while a preceding mail write it issued is
still in flight, because it is the same executor issuing both, one after
the other. No separate wait beyond ordinary FIFO peek/ack sequencing is
needed for that case.

**Across a generation replacement, ordering safety is established before
the replacement is ever admitted, not by the raw write waiting on its own.**
`Consumer Generation Ownership and Replacement` already requires a positive
`GenerationFence` verdict for the outgoing generation before a replacement
is admitted at all. By the time a replacement generation's delivery-loop
executor calls its first `peek`, any effect the outgoing generation's
in-flight write might still have produced has already been positively
observed to have ceased. A raw entry therefore needs no fence wait of its
own beyond the one `peek`/`ack` and generation replacement already provide.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** a peeked raw entry's target transport is `acp`
- **THEN** the transport dispatches via the existing shared ACP
  worker/client `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false` when admitting the raw entry
- **AND** the transport appends Enter after injected text

#### Scenario: Raw is not peekable ahead of older mail

- **WHEN** a raww is submitted for a target that has older unacked mail
- **THEN** `peek` continues returning that older mail until it is acked
- **AND** the raw entry is not returned by any `peek` call until it is at
  the mailbox head

#### Scenario: A generation replacement does not need its own raw fence wait

- **WHEN** a transport generation is replaced for a target whose mailbox
  head, after replacement, is a raw entry
- **THEN** the replacement generation's `peek` returns that raw entry as
  soon as it is at the head
- **BECAUSE** the positive fence verdict required to admit the replacement
  already establishes that the outgoing generation's writes have ceased

### Requirement: ACP Transport Error Code

Relay SHALL use dedicated error code `transport_unavailable` (in ACP context:
`acp_child_unavailable`) for failures caused by ACP child process write
failures or reader thread exit. This code SHALL be distinguishable from
`internal_unexpected_failure` which covers relay-internal logic errors.

Error code taxonomy addendum:

- `transport_unavailable` — ACP stdin write failure (child process dead or
  pipe broken); caller can retry by requesting a worker reconnect
- `internal_unexpected_failure` — relay-internal logic or lock failure;
  not a transport concern

**The synchronous error code names no delivery outcome, and none is spelled
`transport_unavailable`.** `SendOutcome` has no such variant: a declared
member whose submission fails because the transport cannot be reached — an
ACP stdin write erroring without proof that zero bytes left — cannot
exclude a target-side effect and SHALL resolve `submission_unknown` via
`ack`, exactly as any other undifferentiated submission failure does. A
`queued`-and-undeclared member is governed by a separate policy boundary,
per `delivery-quiescence`'s `Mailbox Ordering and Cursor Lifecycle`:

- it SHALL resolve only on a **positively observed terminal lifecycle state** —
  the transport was shut down, or its generation was torn down without
  replacement — or on a sustained unreachability its transport reports as
  `Unreachable` past `[delivery].unreachable-dwell-ms`;
- because such a member was never declared, it resolves `not_submitted`
  **directly, as a relay-side transition** — not through the guard, which
  has nothing to terminalize for an entry it was never bound to;
- a **transient absence** — a respawn in progress, a generation being replaced, a
  UI subscriber that has disconnected but whose session is still registered —
  SHALL leave members `queued` and undeclared, until the absence resolves into
  the transport's own readiness, into a positively observed teardown, or into
  that sustained unreachability. Nothing converts the waiting itself into an
  outcome; what resolves the unreachable case is the repeated observation, not
  its duration alone.

Direct resolution of an undeclared member on definite teardown SHALL be
serialized against `declare` and generation replacement under the same lock,
per `delivery-quiescence`'s `Revocation Serialized Against In-Flight
Submission Bookkeeping` requirement, so a member cannot be resolved by one
path while a live generation is concurrently declaring it.

#### Scenario: ACP stdin write failure returns transport_unavailable

- **WHEN** relay attempts to write a prompt to ACP stdin
- **AND** the write fails with an I/O error
- **THEN** relay returns error code `transport_unavailable`
- **AND** does not return `internal_unexpected_failure`

#### Scenario: transport_unavailable is distinguishable by MCP consumers

- **WHEN** MCP consumer receives an error response for an ACP send
- **AND** error code is `transport_unavailable`
- **THEN** consumer can infer the ACP process is gone and may retry/reattach
- **AND** can distinguish this from a non-retryable relay-internal failure

#### Scenario: An in-flight submission failure resolves submission_unknown, not an error code

- **WHEN** a declared ACP member's stdin write fails without proof that zero
  bytes left
- **THEN** the member resolves `submission_unknown` via `ack`
- **AND** no `transport_unavailable`-spelled outcome is produced, because
  `SendOutcome` has no such variant

#### Scenario: A respawn in progress leaves undeclared members queued

- **WHEN** a target's transport generation is being replaced
- **AND** queued members for that target were never declared
- **THEN** those members remain `queued` and undeclared
- **AND** they resolve only once the replacement's delivery-loop executor
  peeks, declares, and acks them, or if the generation is instead torn down
  with no replacement

#### Scenario: A torn-down transport without replacement resolves its undeclared members as not_submitted

- **WHEN** a transport is shut down, or its generation is torn down with no
  replacement
- **THEN** its `queued`-and-undeclared members resolve `not_submitted`
  directly
- **AND** the outcome is issued from the positively observed lifecycle state, not
  from elapsed time
- **BECAUSE** such a member was never declared and never bound to a packing
  unit, so there is no guard entry to route it through and no concurrent
  evidence that could still arrive for it

### Requirement: Pty Prompt Probe and Look Shall Not Block a Tokio Worker Thread

`PtyPromptProbe::observe` and `PtyOutputView::look` SHALL NOT call `blocking_send`/`blocking_recv` on a tokio worker thread. The snapshot handshake through the worker thread's `mpsc`/`oneshot` channel SHALL be performed off the async runtime.

A Pty target's own delivery-loop executor, deciding whether to declare and write via `PtyPromptProbe::observe`, SHALL NOT panic with "Cannot block the current thread from within a runtime". This is now an entirely transport-internal check — there is no relay-facing handover-readiness read to trigger it — but the blocking-thread hazard is identical regardless of who calls it.

#### Scenario: Pty's own readiness check does not panic the tokio worker

- **WHEN** a Pty target's delivery-loop executor checks its own readiness via `PtyPromptProbe::observe` before deciding whether to declare and write
- **THEN** the probe completes without panicking the `tokio-runtime-worker` thread
- **AND** the relay logs `relay.send.async.queued` followed by `relay.send.async.completed` (or a structured `runtime_transport_startup_failed` on failure), never a stranded `queued` with no `completed`

> **Note:** `Pty look` uses the same snapshot channel and is fixed by the same `state.rs` change, but is currently unreachable from the relay's look handler; end-to-end verification is a follow-up.
