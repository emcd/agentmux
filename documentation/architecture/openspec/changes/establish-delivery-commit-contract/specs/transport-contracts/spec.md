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

**The template gates authorization, and it gates it for every transport that has
one.** A target that is not prompt-ready SHALL NOT have a batch authorized for
it. This is a change in what the template does: it previously gated injection on
Tmux but on Pty only decided what the sender was told, because Pty had already
written the bytes.

**The template SHALL be evaluated by the transport that owns the target, never by
the relay.** The relay learns the result only as the level it reads through
`is_ready_for_handover`, and MUST NOT interpret `prompt_regex`, inspect pane
output, or compare a cursor column itself.

This is a decoupling boundary, not an implementation preference. Readiness
*determination* is transport-specific by nature and does not generalise: a prompt
regex over a pane tail is meaningless for ACP, whose readiness is the completion
of an earlier turn arriving on the wire protocol with no snapshot to inspect, and
meaningless again for UI, whose readiness is subscriber connectivity. A relay
that evaluated the template would be a relay that knows what a pane is, which the
`transport-abstraction` capability's `Transport Module Boundaries` requirement
forbids.

Readiness *scheduling* — deciding which target to visit, in what order, and when
to authorize — remains relay-owned and is transport-agnostic. The two are
separate concerns and only the second belongs to the relay.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled on
content that is not its prompt. A **cursor mismatch** (`prompt_regex` matched but
the reported cursor is not at `input_idle_cursor_column`) means the prompt frame
is healthy and the operator has input pending.

**Both mean the same thing operationally on every transport — do not authorize
yet — and neither SHALL be treated as evidence that the target has failed.** A
target that is not prompt-ready is a target that is not ready *now*; the reason
is not knowable from the inspected tail. A permission dialog awaiting an
operator, a compose box holding typed input, a coder producing no terminal output
while working, and a hung process all present as a settled non-prompt frame.

The per-transport split that previously let Pty conclude failure from a frame
mismatch is removed. No transport infers a terminal outcome from the template,
and the distinction between the two causes survives only as diagnostic
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
  pane output, or compare a cursor column
- **AND** it authorizes solely on the level the transport reports through
  `is_ready_for_handover`

#### Scenario: A transport with no pane has no template to evaluate

- **WHEN** the target's transport observes readiness from a wire protocol or
  subscriber connectivity rather than pane output
- **THEN** it reports `is_ready_for_handover` from that observation
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

### Requirement: Relay raww transport behavior

Relay raww transport execution SHALL map as follows:

- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using existing shared ACP worker/client path via
  `session/prompt`
- pty target: write `text` to the PTY master; if `no_enter=false`, write the
  terminating newline after it
- ui target: emit `text` as a relay stream event through the transport's injected
  broadcaster closure

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

**Ordering.** Mail and raw are variants of one per-target relay FIFO.

Raw SHALL preserve FIFO: no authorization across a raw barrier, nor younger work
across older. It SHALL wait for **target-side ordering safety** of older mail,
which requires that execution has ceased — not merely that an outcome has become
terminal. A ledger transition to `submission_unknown` does not prove a
still-running submission cannot take effect later, so terminality is the weaker
condition and is not sufficient here.

Target-side ordering safety is established by the generation fence's positive
verdict, which is why the raw barrier is held until that verdict rather than
until the outcome resolves.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

#### Scenario: Raw preserves FIFO against pending mail

- **WHEN** a raww is submitted for a target that has older `Pending` mail
- **THEN** the older mail is authorized first
- **AND** the raw write follows it

#### Scenario: Raw waits for target-side ordering safety, not terminality

- **WHEN** a raww is submitted for a target with an authorized batch already
  executing
- **AND** that batch's outcome has resolved `submission_unknown`
- **THEN** the raw write still waits for the generation fence's positive verdict
- **AND** it does not proceed on the terminal outcome alone

#### Scenario: Terminal outcome does not release the raw barrier

- **WHEN** an older batch resolves `submission_unknown`
- **AND** its submission execution has not been fenced
- **THEN** a waiting raw write does not proceed on the terminal outcome alone
- **BECAUSE** a terminal outcome resolves the member but does not prove the
  submission cannot still take effect

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

**The synchronous error code and the delivery outcome of the same spelling are
distinct and SHALL NOT be conflated.** The error code is returned at the request
boundary when a call cannot be accepted. The delivery outcome
`transport_unavailable` resolves an already-queued member and is governed by a
policy boundary:

- it SHALL fire only on a **positively observed terminal lifecycle state** — the
  transport was shut down, or its generation was torn down without replacement;
- a **transient absence** — a respawn in progress, a generation being replaced, a
  UI subscriber that has disconnected but whose session is still registered —
  SHALL leave members `Pending`, until the absence resolves into readiness, into
  a positively observed teardown, or into a sustained unreachability its
  transport reports as `Unreachable` past `[delivery].unreachable-dwell-ms`.
  Nothing converts the waiting itself into an outcome; what resolves the third
  case is the repeated observation, not its duration alone.

Otherwise `transport_unavailable` would become another inference from absence,
retired at the transport and reintroduced at the relay.

Selection between a terminal and a recovering lifecycle state SHALL be serialized
with queue scheduling, so a member cannot be resolved `transport_unavailable` by
one path while another is scheduling it against a live generation.

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

#### Scenario: A respawn in progress does not resolve transport_unavailable

- **WHEN** a target's transport generation is being replaced
- **AND** queued members are `Pending` for that target
- **THEN** those members remain `Pending`
- **AND** they resolve only if the replacement completes and they are authorized,
  or if the generation is instead torn down with no replacement

#### Scenario: A torn-down transport without replacement resolves its pending members

- **WHEN** a transport is shut down, or its generation is torn down with no
  replacement
- **THEN** its `Pending` members resolve `transport_unavailable`
- **AND** the outcome is issued from the positively observed lifecycle state, not
  from elapsed time

## REMOVED Requirements

### Requirement: ACP Prime Timeout

**Reason:** the prime timeout bounds "no observable output has been produced",
which is an absence, and on fire it latches readiness to `Unavailable` and
signals respawn — a terminal judgement about target health drawn from silence.
Under this change ACP performs no wait after authorization: a delivery resolves
at its framed `session/prompt` write, and the turn's later completion is
post-submission target state that does not hold a delivery outcome open.

**Migration:** the `[coders.<id>.acp].prime-timeout-ms` key is deleted outright,
so a `coders.toml` that still sets it fails load on existing unknown-field
validation. An operator SHALL delete the line; no value preserves the prior
behavior. Deliveries that previously resolved `Timeout` with
`reason_code = "acp_turn_timeout"` now resolve `delivered` at the framed write, or
remain `Pending` while the target is reachable but never becomes ready to be
authorized — resolving only if it goes unreachable past the dwell. No outcome
replaces the timeout on that path, because the timeout's outcome was the
inference being retired.

### Requirement: Tmux Prime Timeout

**Reason:** same unsound inference — a bound on the absence of observable output.
With readiness waiting moved relay-side, Tmux performs no prime wait for the
prime timeout to bound.

**Migration:** the `[coders.<id>.tmux].prime-timeout-ms` key is deleted outright
and fails load on existing unknown-field validation. Deliveries that previously
resolved `Timeout` at the prime window now remain `Pending` until the pane becomes
ready, which withholds the claim about the pane that the timeout was making.

### Requirement: Pty Prime Timeout

**Reason:** same unsound inference, and on Pty it was additionally reporting
non-delivery for bytes already written to the master, because Pty wrote before
its wait. Under this change Pty buffers then writes, and its delivery resolves
from the `write_all` pair's own evidence.

**Migration:** the `[coders.<id>.pty].prime-timeout-ms` key is deleted outright
and fails load on existing unknown-field validation. Senders that previously
received `Timeout` for a message that had in fact been written now receive
`delivered`.

### Requirement: Pty Wedged State Detection

**Reason:** the classifier concludes a terminal failure from a settled
non-matching frame, which cannot distinguish a hung coder from a permission
dialog, a compose box, or a coder working silently. It was retained as a named
temporary exception only because it was Pty's sole terminal path pending a Pty
readiness bound. That framing presumed some bound had to supply the terminal path;
under this change none does, and Pty resolves from typed submission evidence once
a batch is authorized. The exception's premise is retired along with it.

Retiring it also removes the masking effect that hid
`agentmux:issues/relay/62`: wedge detection resolved Pty groups within roughly
150 ms, which concealed the coalesce-during-wait message loss. The loss is fixed
by the write reordering in the same change, so the mask and the defect are
retired together rather than sequenced apart.

**Migration:** the `[coders.<id>.pty].wedge-detection` key is deleted outright, so
a `coders.toml` that still sets it fails load on existing unknown-field
validation. An operator SHALL delete the line. Deliveries that previously
resolved `Failed` with `reason_code = "pane_wedged"` now resolve from typed
submission evidence when a batch was authorized, and otherwise remain `Pending`.
