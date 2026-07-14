## MODIFIED Requirements

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
  ANY terminal classification;
- reset the consecutive-mismatch counter the wedge classifier uses, so
  any wedged-counter progress accumulated during a prior quiesced period
  is cleared when terminal output resumes;
- emit a `delivery_target_active` diagnostic inscription carrying
  `target_session`, `pane_target` (when the probe surfaces one), and
  `activity_delta` (the magnitude of the activity generation advance).
  The diagnostic dedups by generation: an iteration whose activity
  generation did not advance does not emit a duplicate.

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
constant `activity_generation` value across observations, the
comparator never registers an advance, and the wedge classifier
continues to fire `pane_wedged` on such a target — the same false
positive the change was supposed to prevent. The silent-thinking case
is a real bug but requires a separate process-level aliveness
signal (filed as a follow-up); it is out of scope for this change.

**Branch ordering contract.** The Busy short-circuit SHALL be
evaluated before any terminal-classification branch in the same
observe-sleep-observe iteration. The required branch order in
`quiescence_classify_step` (in `src/transports/quiescence.rs`), after
the second observation capture, is:

1. `operator_interaction_active` check (operator copy-mode /
   key-table) — suppresses all classification.
2. **Busy short-circuit** (the new branch from this proposal) — when
   the activity generation advanced, reset the wedge counter, emit
   `delivery_target_active`, return `NeedsWait`.
3. `delivery_ready` check — terminal: returns
   `Done(Ok(snapshot_after.pane_target.unwrap_or_default()))` when
   the snapshot is prompt-ready.
4. Wedge-counter increment block.
5. Wedge check (counter threshold or prime-window elapsed) —
   `Done(Err(Wedged))`.
6. Prime timeout check — `Done(Err(Timeout))`.

This ordering is what implements the Busy-suppresses-all-terminal-
classifications behavior above. In particular, Busy SHALL be
evaluated BEFORE `delivery_ready`; a post-sleep observation that
matches the prompt regex while the activity generation advanced
during the same quiet window SHALL return `NeedsWait` (Busy), not
`Done(Ok(...))` (Delivered). The wedge counter SHALL only advance
during iterations in which the activity signal was also quiesced;
this is an implicit guard from Busy returning early at step 2.

The `unresponsive` and `wedged` classifiers SHALL each be config-surfaced
per the per-transport spec (see `session-relay` Tmux Prime Timeout and
Tmux Wedged State Detection requirements for the Tmux surface).

- The Tmux `unresponsive` classifier SHALL be **opt-in**: absent or
  `None` on `[coders.<id>.tmux].prime-timeout-ms` preserves today's
  unbounded behavior.
- The Tmux `wedged` classifier SHALL be **opt-out**: it defaults to
  enabled (`wedge-detection` is `true` when absent or `true`),
  because the cost of a silently-wedged pane is higher than the cost
  of a false-positive wedge. Operators MAY set
  `[coders.<id>.tmux].wedge-detection = false` to preserve the prior
  unbounded-wait behavior.

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
- **AND** the relay worker treats the resulting `SingleDeliveryOutcome`
  as terminal regardless of which classifier fired

#### Scenario: Tmux wedge detection defaults to enabled

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **THEN** the Tmux transport classifies a settled, non-prompt-ready,
  no-operator-interaction pane as `wedged`
- **AND** resolves the flush group as `Failed` with
  `reason_code = "pane_wedged"`

#### Scenario: Tmux wedge detection opt-out preserves prior behavior

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`
  (if relay shutdown is requested)

#### Scenario: Tmux prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not fire `Timeout` for unresponsive
  targets regardless of how long output remains absent
- **AND** the only terminal failure modes for the flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default) and `Shutdown`

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

#### Scenario: Busy short-circuit suppresses wedged classification on active target

- **WHEN** the Tmux transport's quiescence wait observes the target's
  activity signal advancing between two consecutive observation polls
- **AND** the inspected pane tail does not match the prompt-readiness
  template (the screen has not yet returned to the prompt because the
  target is mid-generation)
- **THEN** the transport does NOT classify the flush group as `wedged`
- **AND** continues to wait for either the activity to settle and the
  pane to become prompt-ready, or the prime window to elapse with no
  activity observed
- **AND** emits a `delivery_target_active` diagnostic inscription
  carrying the activity delta

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

#### Scenario: Busy short-circuit resets wedge counter

- **WHEN** the wedge counter has accumulated to one or two consecutive
  identical quiesced-mismatch signatures
- **AND** the next observation reports an activity-signal advance
- **THEN** the wedge counter is reset to zero
- **AND** the counter starts accumulating again only after the activity

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
  signal quiesces and the pane content remains settled at a non-prompt
  state

## ADDED Requirements

### Requirement: Positive Activity Signal

Each promptable transport that owns a quiescence wait SHALL populate the
cross-transport `WedgeObservation.activity_generation` field on every
call to `WedgeProbe::observe` from a transport-native
**terminal-output-write** primitive. The classifier compares this
field across two consecutive observations to detect "did bytes flow
between these two polls" independently of whether the captured
pane/screen content visibly changed. The activity generation is a
monotonic `u64`; the classifier treats an advance as a positive
"terminal-output-write" signal.

**Scope (terminal-output-write, not process-busy):** the field carries
a marker of "bytes being written to the target's terminal." It does
NOT carry a marker of "the target's agent is busy regardless of byte
output" — that is a separate problem requiring a process-level
aliveness signal (filed as a follow-up). A target whose agent is in
silent thinking with zero terminal bytes will populate this field
with a constant value, and the wedge classifier will continue to
fire on it as before.

The transport-native activity primitive SHALL be:

- **Tmux**: `#{window_activity}` (the same primitive
  `RealPaneQuiescenceProbe::wait_for_change` already polls). The Tmux
  probe resolves the marker at observation time and parses it as a
  `u64` epoch-seconds value. When `#{window_activity}` is unavailable
  on the running tmux version (the existing
  `resolve_window_activity_marker` returns `Ok(None)` for unknown /
  invalid / bad format errors), the field SHALL be populated with `0`,
  falling back to pre-change behavior for older tmux versions.

- **Pty**: `last_change_atomic` on `PtyShared` (the `Arc<AtomicU64>`
  the worker thread advances after each `vt_write` batch). The Pty
  probe loads the atomic with `Ordering::Acquire`. The field is
  already `u64`; no parsing needed.

The activity signal SHALL be transport-internal: the field is part of
the cross-transport `WedgeObservation` type but does NOT appear in
`DeliveryEnvelope`, `SingleDeliveryOutcome`, or any relay-facing API.
A transport that does not track activity (or whose primitive is
unavailable) populates the field with a constant (`0`), which falls
back to the pre-change behavior for that transport.

#### Scenario: Tmux probe populates activity_generation from window_activity

- **WHEN** the Tmux `TmuxAsWedgeProbe::observe` is called and
  `#{window_activity}` returns a non-empty value
- **THEN** the resulting `WedgeObservation.activity_generation` is the
  parsed `u64` epoch-seconds value of that marker

#### Scenario: Tmux probe falls back to 0 when window_activity is unavailable

- **WHEN** the Tmux `TmuxAsWedgeProbe::observe` is called and
  `#{window_activity}` is unavailable on the running tmux version
- **THEN** the resulting `WedgeObservation.activity_generation` is `0`
- **AND** the classifier's Busy short-circuit never fires for this
  probe (no activity advance is possible when the field is always `0`)

#### Scenario: Pty probe populates activity_generation from last_change_atomic

- **WHEN** the Pty `PtyQuiescenceProbe::observe` or
  `WorkerTerminalProbe::observe` is called
- **THEN** the resulting `WedgeObservation.activity_generation` is the
  current value of `last_change_atomic` loaded with `Ordering::Acquire`

#### Scenario: Classifier compares activity_generation between observations

- **WHEN** two consecutive `WedgeObservation` snapshots have different
  `activity_generation` values
- **THEN** the classifier recognizes the second observation as
  reporting activity since the first
- **AND** enters the Busy pre-classification for that iteration