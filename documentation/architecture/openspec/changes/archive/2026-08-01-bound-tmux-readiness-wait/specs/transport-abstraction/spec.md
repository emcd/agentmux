## MODIFIED Requirements

### Requirement: Three-State Delivery Classifier

Promptable transports that gate delivery on a quiescence wait SHALL classify
each pending flush group, during the quiescence wait for that group, into
one of three terminal states:

- `running` — output is flowing or has settled at the prompt-readiness
  match; the transport continues to wait normally and resolves the flush
  group as `Delivered` when the prompt becomes ready.
- `unresponsive` — during the quiescence wait for the flush group, no
  observable output has been produced within the prime window, or the flush
  group's readiness bound elapsed without the target becoming ready; the
  transport resolves the flush group as `SendOutcome::Timeout`.
- `wedged` — during the quiescence wait for the flush group, output has
  settled and the prompt-readiness template's frame does not match; the
  transport resolves the flush group as `SendOutcome::Failed` with a
  transport-defined `reason_code` on the same `Failed` variant.

The `wedged` classification is **unsound**. It infers a terminal failure from the
absence of change in rendered content, which cannot distinguish a hung target
from a permission dialog awaiting an operator, a compose box holding typed input,
or a target working without output.

Tmux SHALL NOT classify `wedged`. **Pty is the sole retained user of the
classification, as a named temporary exception**: it is Pty's only terminal path
today, and removing it before `agentmux:issues/relay/61` supplies a Pty readiness
bound would leave Pty unable to end a wait at all. No other transport SHALL adopt
`wedged`, and a transport lacking a readiness bound SHALL NOT read that lack as
licence to classify `wedged`. Missing a bound is the condition that has so far
kept Pty from dropping an unsound classification, not a criterion that makes the
classification sound; the remedy is to supply the bound.

Positive observation of activity remains a valid signal on every transport and
continues to suppress injection. **For Tmux**, the absence of activity SHALL NOT
be treated as a signal: only a positively observed terminal event — process
death, a closed connection, a protocol error — is sound evidence of failure, and
an unchanged screen is not. Pty's retained `wedged` classifier does infer failure
from an unchanged screen; that it does is the defect `agentmux:issues/relay/61`
closes, not an exemption this rule grants.

Tmux exposes `pane_dead`, but only under `remain-on-exit`, which this system
does not set; without it a dead process destroys the pane and the resulting
probe failure already resolves the wait. That path is therefore left unbuilt
deliberately rather than overlooked.

A Tmux quiescence wait SHALL be bounded by the flush group's readiness bound
(`DeliveryEnvelope.readiness_timeout_ms`; see the `delivery-quiescence`
capability's `Quiescence-Gated Delivery` requirement). The bound covers the
entire wait for the group, is anchored where the prime window is anchored, and
SHALL NOT be deferred, extended, or suspended by any signal. When it elapses the
classifier SHALL promote the flush group to a terminal state, selecting the
outcome and reason from the most recent observation.

Transports that receive no readiness bound are unaffected by the preceding
paragraph. This requirement does not bound their waits, and the absence of a
bound for them SHALL NOT be read as their being bounded by other means; see
`agentmux:issues/relay/61`.

In addition to the three terminal classifications above, the classifier
SHALL recognize a non-terminal **Busy** pre-classification: when the
target's positive terminal-output-write signal (see the `Positive
Activity Signal` requirement) advances between two consecutive
observation polls, the classifier SHALL:

- treat the target as **Busy** for that iteration;
- suppress all terminal classifications for that iteration —
  `running` (Delivered), `unresponsive` (Timeout), AND `wedged`
  (Failed) — regardless of what the readiness-prompt match or the
  inspected-pane-tail emptiness says. While the
  terminal-output-write signal continues to be reported across
  iterations, the classifier SHALL NOT promote the flush group to
  ANY terminal classification **for as long as the flush group's readiness
  bound has not elapsed**;
- reset the consecutive-mismatch counter the wedge classifier uses, so
  any wedged-counter progress accumulated during a prior quiesced period
  is cleared when terminal output resumes;
- emit a `delivery_target_active` diagnostic inscription carrying
  `target_session`, `pane_target` (when the probe surfaces one), and
  `activity_delta` (the magnitude of the activity generation advance).
  The diagnostic dedups by generation: an iteration whose activity
  generation did not advance does not emit a duplicate.

Where a readiness bound applies, Busy suppression is bounded rather than
indefinite. The wedge-counter reset above is retained deliberately: the wedge
condition requires continuous frame-absence, so counter progress accumulated
before an activity burst SHALL NOT survive it, or a stale wedge start would
combine with newly settled content and fire immediately. Bounding the wait is
the readiness bound's responsibility, not the counter's.

The `Busy` pre-classification SHALL NOT be surfaced as a terminal
classification. The three terminal classifications remain `running`,
`unresponsive`, and `wedged`; `Busy` is the classifier's way of
saying "keep waiting, the target is alive" without committing to a
terminal outcome.

**Scope clarification.** The `Busy` pre-classification is triggered
ONLY by the terminal-output-write signal — that is, bytes being
written to the target's pane/screen. It does NOT trigger when the
target's agent process is busy but is producing zero terminal bytes
(e.g., silent model thinking, pre-output tool-call prep). This
distinction is explicit: a target in silent thinking produces a
constant `activity_generation` value across observations and the
comparator never registers an advance. On a transport that still
classifies `wedged` — Pty today — that target is reported as wedged,
which is the false positive the Busy short-circuit was meant to
prevent and cannot. On Tmux the case is now benign: the target simply
remains pending until it produces output or the readiness bound
elapses. This is one of the four indistinguishable cases that motivated
removing the Tmux classifier, and it is why Pty's removal is tracked
alongside its bound. The silent-thinking case
is a real bug but requires a separate process-level aliveness
signal (filed as a follow-up); it is out of scope for this change.

**Branch ordering contract.** The Busy short-circuit SHALL be
evaluated before any terminal-classification branch in the same
observe-sleep-observe iteration. The required branch order in
`quiescence_classify_step` (in `src/transports/quiescence.rs`), after
the second observation capture, is:

1. **Busy short-circuit** — when the activity generation advanced,
   reset the wedge counter, emit `delivery_target_active`, return
   `NeedsWait`.
2. `delivery_ready` check — terminal: returns
   `Done(Ok(snapshot_after.pane_target.unwrap_or_default()))` when
   the snapshot is prompt-ready.
3. Wedge-counter increment block.
4. Wedge check (counter threshold or prime-window elapsed) —
   `Done(Err(Wedged))`.
5. Prime timeout check — `Done(Err(Timeout))`.

This ordering is what implements the Busy-suppresses-all-terminal-
classifications behavior above. In particular, Busy SHALL be
evaluated BEFORE `delivery_ready`; a post-sleep observation that
matches the prompt regex while the activity generation advanced
during the same quiet window SHALL return `NeedsWait` (Busy), not
`Done(Ok(...))` (Delivered). The wedge counter SHALL only advance
during iterations in which the activity signal was also quiesced;
this is an implicit guard from Busy returning early at step 1.

The readiness bound is **not** a branch in this ordering. It is a precondition
on the iteration's result: an iteration whose readiness bound has elapsed SHALL
NOT return `NeedsWait` from any branch above, and any `NeedsWait` deadline the
classifier returns SHALL be capped at the bound. Expressing the bound as a
positional early return would be incorrect, because it would report the
readiness outcome in an iteration where a higher-precedence outcome is
available. The classifier SHALL evaluate every elapsed bound before selecting an
outcome and then apply precedence.

An elapsed readiness bound SHALL NOT pre-empt the `delivery_ready` branch. When
the bound has elapsed and the observation is prompt-ready and the activity
signal did not advance, the classifier SHALL resolve `Delivered`. Full outcome
precedence within one iteration, highest first: delivery, then the prime
timeout, then the readiness bound. Delivery ranks first because the bound exists
to stop an unbounded wait rather than to refuse a success that is already
available, and because the prompt-ready check already precedes the prime-timeout
check in the branch order above. On a transport that still classifies `wedged` —
Pty today — that classification sits between delivery and the prime timeout.

The Busy pre-classification is unaffected by this. A prompt-ready observation
whose activity generation advanced across the pair is still deferred rather than
delivered; if the bound has elapsed in that iteration the group resolves
terminally instead of returning `NeedsWait`.

The `unresponsive` and `wedged` classifiers SHALL each be config-surfaced
per the per-transport spec (see `transport-contracts` Tmux Prime Timeout for the
Tmux surface and Pty Wedged State Detection for the Pty surface).

- The Tmux `unresponsive` classifier's prime window SHALL be **opt-in**: absent
  or `None` on `[coders.<id>.tmux].prime-timeout-ms` means no prime-window
  verdict is issued. It does not mean the wait is unbounded; the Tmux readiness
  bound still applies.
- Tmux SHALL NOT surface a `wedged` knob. Tmux does not classify `wedged` at all,
  so there is no behavior for a knob to select.
- The Pty `wedged` classifier SHALL remain **opt-out**, defaulting to enabled,
  until `agentmux:issues/relay/61` supplies a Pty readiness bound and the
  classifier is removed with it. It is retained because it is Pty's only terminal
  path, not because the classification is sound.

No signal that the transport cannot bound SHALL suppress, defer, or otherwise
gate a Tmux classification indefinitely. This applies to operator-observable
rendering state on the Tmux transport — copy-mode or a non-`root` client
key-table — which does not change what `capture-pane` or `cursor_x` report and
does not impede injection (see the `transport-contracts` `Copy-Mode-Transparent
Injection` requirement), so such states are not delivery preconditions. It
applies equally to the terminal-output-write signal, which is likewise unbounded:
a target may emit bytes indefinitely without ever becoming ready. A Tmux
quiescence wait SHALL always progress toward one of its terminal
classifications, and the readiness bound is the mechanism that guarantees it.
(This does not affect the ACP transport's `pending_choice_outcome` pause, which
is a distinct turn-blocking operator *decision* rather than a signal the
transport cannot bound.)

The classifier SHALL be evaluated at the transport's quiescence wait,
NOT at the relay delivery worker. The relay SHALL NOT inspect
`SingleDeliveryOutcome` to make delivery policy decisions; it only
relays the outcome to the MCP/CLI caller and to the diagnostic stream.

The three states are mutually exclusive at the moment of terminal
classification. The classifier SHALL NOT combine them (for example, a
flush group SHALL NOT resolve as `Timeout AND Failed`).

#### Scenario: Tmux pane-content observation resolves to delivery or timeout

- **WHEN** the Tmux transport's quiescence wait observes the target's
  output state during the wait for a flush group
- **THEN** the outcome it derives from that observation is exactly one of
  `Delivered` or `Timeout`
- **AND** no Tmux outcome carries `reason_code = "pane_wedged"`
- **AND** the transport's terminal paths that are not derived from pane content —
  relay shutdown, and a positively observed probe or transport failure — are
  unaffected by this scenario
- **AND** the relay worker treats the resulting `SingleDeliveryOutcome`
  as terminal regardless of which classifier fired

#### Scenario: A settled Tmux pane is not classified as failed

- **WHEN** a Tmux pane is quiescent with the prompt frame absent, for any reason
  — a hung coder, a permission dialog awaiting an operator, a compose box
  holding typed input, or a coder working without terminal output
- **THEN** the Tmux transport issues no terminal outcome on that basis
- **AND** the flush group remains pending until the target becomes ready or the
  readiness bound elapses
- **AND** an elapsed prime timeout does not resolve the group either: the prime
  window measures absence of observable output, and a settled frame is output
- **BECAUSE** the four cases are indistinguishable from the inspected tail, so
  classifying any of them as failed misreports three of them, and resolving them
  on the prime timeout instead would draw the same inference under another name

#### Scenario: Absent Tmux prime timeout suppresses the prime verdict, not the bound

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not fire a prime-window `Timeout` for
  unresponsive targets regardless of how long output remains absent
- **AND** the flush group's readiness bound still applies
- **AND** the terminal failure modes for the flush group are `Timeout` from the
  readiness bound and `Shutdown`

#### Scenario: Classification is unaffected by operator copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the operator
  scrolled it with the mouse wheel)
- **THEN** the classifier evaluates prompt-readiness against the pane's live
  content, which copy-mode does not alter
- **AND** a prompt-ready pane resolves as `Delivered`
- **AND** the transport does NOT suppress or defer classification on account
  of the copy-mode state

#### Scenario: Group atomicity on failure classification

- **WHEN** a promptable transport's quiescence wait classifies the flush group
  into a non-delivered terminal state — `unresponsive` on either transport, or
  `wedged` on Pty
- **THEN** every sender in the flush group receives the same terminal
  outcome
- **AND** the transport does NOT classify individual envelopes
  independently within the same flush group

#### Scenario: Busy short-circuit suppresses terminal classification on an active Tmux target

- **WHEN** the Tmux transport's quiescence wait observes the target's
  activity signal advancing between two consecutive observation polls
- **AND** the inspected pane tail does not match the prompt-readiness
  template (the screen has not yet returned to the prompt because the
  target is mid-generation)
- **AND** the flush group's readiness bound has not elapsed
- **THEN** the transport does NOT promote the flush group to any terminal
  classification for that iteration
- **AND** continues to wait for either the activity to settle and the
  pane to become prompt-ready, or the prime window to elapse with no
  activity observed, or the readiness bound to elapse
- **AND** emits a `delivery_target_active` diagnostic inscription
  carrying the activity delta

#### Scenario: Delivery outranks a simultaneous readiness expiry

- **WHEN** the flush group's readiness bound elapses in the same iteration in
  which the observation is prompt-ready
- **AND** the activity generation did not advance across the observation pair
- **THEN** the classifier resolves `Delivered` and the message is injected
- **AND** it does not resolve `Timeout` on account of the elapsed bound

#### Scenario: Busy suppression ends at the readiness bound

- **WHEN** a Tmux target's activity signal advances on every observation pair
- **AND** the prompt-readiness template never matches
- **AND** the flush group's readiness bound elapses
- **THEN** the transport promotes the flush group to a terminal classification
  rather than returning `NeedsWait` again
- **AND** the outcome is `Timeout` with `reason_code = "target_never_settled"`
- **BECAUSE** the terminal-output-write signal is unbounded, and suppression
  keyed to an unbounded signal is an unbounded wait

#### Scenario: Pty busy short-circuit suppresses wedged classification on active target

- **WHEN** the Pty transport's quiescence wait observes the worker's
  `last_change_atomic` advancing between two consecutive observation
  polls (new bytes were applied to the libghostty-vt terminal)
- **AND** the inspected screen tail does not match the prompt-readiness
  template
- **THEN** the transport does NOT classify the flush group as `wedged`
- **AND** continues to wait for either the activity to settle and the
  screen to become prompt-ready, or the prime window to elapse with no
  activity observed

#### Scenario: A transport without a readiness bound is unaffected

- **WHEN** a flush group's `readiness_timeout_ms` is `None`
- **THEN** the classifier applies no readiness bound to that group
- **AND** its wait behavior is exactly as it was before this requirement gained
  the bound
- **BECAUSE** the shared state machine is used by transports whose readiness
  contracts differ, and sharing the code path SHALL NOT impose a bound the
  transport's own requirement has not defined

#### Scenario: Busy short-circuit resets wedge counter

- **WHEN** the wedge counter has accumulated to one or two consecutive
  identical quiesced-mismatch signatures
- **AND** the next observation reports an activity-signal advance
- **THEN** the wedge counter is reset to zero
- **AND** the counter starts accumulating again only after the activity
  signal quiesces and the pane content remains settled at a non-prompt
  state
- **AND** the flush group's readiness bound is unaffected by the reset

#### Scenario: Busy short-circuit defers Delivered during active output (branch-ordering contract)

- **WHEN** the post-sleep observation matches the prompt-readiness
  template (the snapshot would normally resolve the wait as
  `Done(Ok(...))` via the `delivery_ready` branch)
- **AND** the activity generation advanced between the two consecutive
  observation polls
- **THEN** the classifier fires the `Busy` short-circuit (returns
  `NeedsWait`) rather than the `delivery_ready` branch
- **AND** the wedge counter is reset to zero
- **AND** the wait function continues to the next iteration, where
  the `delivery_ready` check will resume only after the activity
  generation has settled AND the snapshot continues to match the
  prompt-readiness template across a consecutive observation pair
- **AND** the classifier does NOT promote the flush group to
  `Delivered` while activity is being reported, even momentarily
  when the snapshot happens to match the prompt regex

This scenario exists to make the branch-ordering contract testable
in `tests/unit/tmux_transport.rs` and `tests/unit/pty_transport.rs`:
a probe that advances `activity_generation` between observations
while keeping `is_prompt_ready == true` MUST resolve as
`QuiescenceAction::NeedsWait`, NOT `Ok(pane)`, until the activity
generation quiesces across an observation pair.

### Requirement: Generalized Wedge/Prime State Machine

The system SHALL provide a transport-agnostic wedge detection and prime
timeout state machine in `src/transports/quiescence.rs`, shared by all
promptable transports (Tmux, Pty). The state machine SHALL operate over
a `WedgeProbe` trait that exposes a single-snapshot observation shape:

- `observe(&mut self) -> Result<WedgeObservation, String>` — captures
  the probe's current state as a single snapshot. The state machine
  calls this twice per quiescence iteration (before and after the
  `wait_for_change` round). Implementations read any underlying IPC /
  state once and return a consistent snapshot.
- `wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>`
  — blocks until the next `observe()` call would differ from the
  previous one, or the supplied `deadline` elapses. Returns `Ok(())`
  on observed change; `Err(DeliveryWaitError::Timeout)` on deadline
  elapsed with no change; `Err(DeliveryWaitError::Failed)` on probe
  errors. The state machine SHALL pass the earliest applicable bound as the
  `deadline`: the per-coder `prime_timeout_ms` when set, and the flush group's
  readiness bound when one applies, whichever is sooner. A supplied deadline
  SHALL NOT exceed a bound that applies to the group.

The single-snapshot shape is intentional: a multi-method trait
would do 4-8x more work per iteration when the probe side-effects
on each call, and the existing probe test fixtures'
`abort_after_calls` counters would trip prematurely. The 16-probe
test surface in `tests/unit/tmux_transport.rs` uses this two-method
shape and preserves its `next_evaluation` cadence.

The `WedgeObservation` snapshot SHALL carry these fields (consistent
across all transports; per-transport probes populate them from their
native primitives):

- `inspected_tail: String` — the last `inspect_lines` rows formatted
  for prompt-readiness matching. Empty / whitespace-only indicates
  an empty pane (Unresponsive territory). A non-empty tail that is not
  prompt-ready is wedge-class only when the mismatch is a frame mismatch;
  see `mismatch` below.
- `is_prompt_ready: bool` — whether the target is currently
  prompt-ready. The state machine's `running` branch returns `Ok`
  when this is `true`.
- `pane_target: Option<String>` — active pane id (e.g. Tmux `%0`)
  for diagnostic inscriptions. `None` when the probe does not
  surface a pane target (e.g. Pty, which has no tmux-style pane
  id); the state machine omits the field from diagnostics in that
  case.
- `mismatch: Option<ReadinessMismatch>` — readiness-mismatch
  metadata when `is_prompt_ready = false`. The state machine uses
  `mismatch.reason` for the wedge/prime-timeout `reason` payload,
  falling back to deriving a generic reason from the inspected tail
  when `None`. Its `regex_matched` field SHALL determine wedge-class
  membership: a mismatch reported with `regex_matched = Some(true)` is a
  cursor mismatch on a healthy prompt frame and SHALL NOT be wedge-class,
  because it indicates pending operator input rather than a stuck target.
- `activity_generation: u64` — terminal-output-write marker
  populated at observation time. Tmux probes read
  `#{window_activity}` parsed as a `u64` epoch-seconds value
  (falling back to `0` when the format is unavailable on the
  running tmux version). Pty probes read
  `last_change_atomic.load(Ordering::Acquire)` from `PtyShared`. An
  advance between two consecutive observations signals that
  bytes were written to the target during the `quiet_window`,
  triggering the `Busy` pre-classification (see the
  `Three-State Delivery Classifier` requirement).

The state machine SHALL return the existing
`DeliveryWaitError::{Timeout, Wedged}` variants declared in
`src/transports/contract.rs`. Tmux and Pty SHALL share the state
machine; the per-transport adapter is the only divergence. The
Tmux transport constructs a small `TmuxAsWedgeProbe` adapter that
maps the existing `PaneQuiescenceProbe` into the new generalized
trait, preserving the 16-probe test surface in
`tests/unit/tmux_transport.rs` unchanged. The Pty transport
implements `WedgeProbe` directly in
`src/pty/state.rs::{PtyQuiescenceProbe, WorkerTerminalProbe}`,
populating `WedgeObservation` fields from a shared `PtyShared`
handle.

Sharing the state machine SHALL NOT impose one transport's bounds on another.
A bound applies to a flush group only when that group's envelope carries it.

#### Scenario: Generalized state machine classifies based on probe results

- **WHEN** the shared wedge/prime state machine observes a flush
  group whose probe reports `is_prompt_ready == false` with a frame
  mismatch (prompt-readiness template does not match the inspected tail)
- **AND** wedge detection is enabled (per-coder config)
- **THEN** the state machine returns `DeliveryWaitError::Wedged { reason }`
  after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
  evaluations, OR when the prime window has elapsed with a
  wedge-class mismatch observed
- **AND** the calling transport maps the error to
  `SendOutcome::Failed` + `reason_code = "pane_wedged"`

#### Scenario: A cursor mismatch does not accumulate wedge evaluations

- **WHEN** the probe reports `is_prompt_ready == false` with a mismatch whose
  `regex_matched` is `Some(true)`
- **THEN** the evaluation is not wedge-class
- **AND** the consecutive-mismatch counter does not advance for it
- **AND** the state machine does not return `Wedged` regardless of how many such
  evaluations occur

#### Scenario: Tmux adapter maps PaneQuiescenceProbe into WedgeProbe::observe

- **WHEN** a Tmux-backed flush group's `TmuxAsWedgeProbe::observe`
  is called
- **THEN** the adapter invokes the underlying `PaneQuiescenceProbe`
  exactly once and packages the result into a `WedgeObservation`
  whose fields (`inspected_tail`, `is_prompt_ready`, `pane_target`,
  `mismatch`, `activity_generation`) reflect the live pane state at
  the moment of the call
- **AND** the Tmux-side prime and Busy semantics match the merged
  `tmux-wedge-detection` and `add-wedge-detection-busy-state`
  proposals unchanged, except that Tmux no longer consumes the
  state machine's `Wedged` result

### Requirement: Transport-Internal Probe Seam for Testability

Each promptable transport that owns a quiescence wait SHALL expose an
internal probe trait that lets tests inject deterministic quiescence and
prompt-readiness results. The probe trait SHALL be transport-internal (not
part of the `Transport` contract) and SHALL NOT appear in
`src/transports/contract.rs`.

The probe trait SHALL return the next observation on demand so tests can
drive the classifier through specific sequences. The sequences a transport's
tests SHALL cover are the terminal states that transport can actually reach:
for Tmux, unresponsive, slow-prompt, and normal-flow; a wedged sequence is
not among them, because Tmux no longer classifies `wedged`.

#### Scenario: Tmux probe trait is transport-internal

- **WHEN** a developer reads `src/tmux/quiescence_probe.rs`
- **THEN** they find a `PaneQuiescenceProbe` trait used by
  `wait_for_quiescent_pane_three_state`, both re-exported from `src/tmux/mod.rs`
- **AND** the trait is not re-exported from `src/transports/`
- **AND** the `Transport` trait in `src/transports/contract.rs` has no
  knowledge of probes

#### Scenario: Tmux unit tests cover the reachable canonical sequences

- **WHEN** `cargo nextest run --test unit tmux_transport` runs
- **THEN** it asserts the canonical probe sequences produce the
  expected wait results:
  - a probe that never produces output → `Err(DeliveryWaitError::Timeout)`
  - a probe that quiesces at a prompt after several ticks → `Ok(pane_target)`
  - a probe that produces output then settles at a prompt →
    `Ok(pane_target)` without the prime timeout firing
- **AND** no Tmux sequence asserts `Err(DeliveryWaitError::Wedged)`, which the
  transport cannot produce

#### Scenario: Readiness-bound coverage lives with the shared classifier

- **WHEN** a developer looks for the tests covering the Tmux readiness bound
- **THEN** they find them against the shared classifier both transports drive,
  not duplicated per transport
- **BECAUSE** the bound is applied by the shared state machine, and asserting it
  through one transport's probe adapter would test the adapter rather than the
  rule
