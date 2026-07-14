# Design: Wedge detection — positive busy signal short-circuits wedged classification

## Context

The wedge/prime quiescence classifier in `src/transports/quiescence.rs`
is the cross-transport shared state machine consumed by every
promptable transport (Tmux and Pty today; future promptable transports
inherit the same seam). It drives the three-state classification
(running / unresponsive / wedged) for each delivery's quiescence wait.

The current classifier's "is the pane alive?" signal is purely
"did the rendered pane/screen content change between two consecutive
observation polls" (`let quiescent = snapshot_before == snapshot_after;`,
where `WedgeObservation` derives `PartialEq`). The classifier has no
positive "busy" signal distinct from snapshot change.

`issues/transports/2` documents the false-positive classification
incidents: a target session that is actively generating a response but
is not producing new visible terminal output during the `quiet_window`
reads as `quiescent = true`, and once the prompt-readiness regex also
fails to match (busy screens don't look like the idle prompt), the
wedge counter accumulates and the classifier fires `pane_wedged`. The
operator's logged case is RG's session, misclassified within ~2.5s of a
send attempt.

The operator's three-state model, distilled from the issue:

- **Busy**: pane/pty activity IS reported. Readiness-prompt match is
  irrelevant — never classify as wedged while activity is present.
- **Wedged**: NO activity reported, AND no match for the readiness
  prompt.
- **Ready**: NO activity reported, AND the readiness prompt DOES
  match, after a quiescence period.

The model's "Busy" state is the missing primitive — but the operator's
"activity" is ambiguous between "terminal-output-write activity"
(Class A) and "agent process busy regardless of byte output" (Class B
— silent thinking). This proposal implements the Busy state with a
**terminal-output-write marker** only, which catches Class A reliably;
Class B is a real but separate bug class filed as a follow-up (see
"Out of scope" below).

## Goals / Non-Goals

- Goals:
  - Add a positive **terminal-output-write** signal to the
    cross-transport classifier so the Class A case of the operator's
    three-state model can be implemented directly.
  - Implement the Busy short-circuit: when terminal-output bytes are
    reported between two observations (Class A), never classify as
    wedged regardless of what the readiness-prompt match says.
  - Preserve existing behavior in the no-output-activity path (the
    baseline five probe classes must continue to resolve as before).
  - Emit a new diagnostic inscription so operators can see when the
    short-circuit fired in the diagnostic stream.
  - Surface the terminal-output-write signal from each transport's
    native primitive; do NOT introduce a transport-agnostic
    abstraction for "what counts as activity" — each transport owns
    its own primitive.

- Non-Goals:
  - Redesigning the wedge-class taxonomy or retuning
    `WEDGE_CONSECUTIVE_TICKS`.
  - Replacing Tmux's `#{window_activity}` with a finer-grained signal
    (e.g., `#{pane_activity}`) — defer until empirical data shows
    the window-level signal is too coarse.
  - Fixing `issues/transports/1` (quiescence-timeout outcome mis-
    report) — separate bug, same neighborhood.
  - Generic wedge-detection wiring for ACP — ACP owns its own wedge
    machinery.

## Decisions

### Decision 1: Use a monotonic u64 `activity_generation` field on `WedgeObservation` (terminal-output-write marker, NOT process-busy marker)

The classifier needs to compare terminal-output-write activity between
two observations to detect "did bytes flow between these two polls".
A monotonic counter (`u64`) captures this naturally:
`activity_advanced = snapshot_before.activity_generation != snapshot_after.activity_generation`.

A boolean would also work (e.g., `output_reported_since_last_poll`),
but the counter has two advantages:

- It carries the magnitude of advance, which surfaces usefully in the
  `delivery_target_active` diagnostic as `activity_delta`.
- It survives saturation: if the underlying activity marker overflows
  (Tmux's epoch-seconds value does not realistically, but defensive
  coding applies), the comparison still works as "did the value
  change".

The field is `u64` rather than `Option<u64>` to keep the comparison
arithmetic simple. A transport that has no activity signal at all can
populate the field with a constant (`0`), which falls back to the
pre-change behavior (the Busy branch is never entered).

**Scope clarification (this decision does NOT cover Class B):** the
field is a **terminal-output-write** marker. It advances when bytes
are written to the terminal; it does NOT advance when the agent is
thinking with zero byte output. A target in silent thinking (Class B)
will populate the field with a constant value, the comparator will
never trigger, and the wedge classifier will continue to fire on the
same false-positive it fires today. See `Risk: Class B silent-thinking
case` below.

### Decision 2: Per-transport activity sources, no shared abstraction

The "what counts as activity" question is transport-specific:

- **Tmux**: `#{window_activity}` (via `resolve_window_activity_marker`
  in `src/tmux/pane.rs`). This is the project's existing activity
  primitive; it's already used by
  `RealPaneQuiescenceProbe::wait_for_change`. Reusing it keeps the
  design within the bounds of what tmux makes observable.
- **Pty**: `last_change_atomic` on `PtyShared` (the `Arc<AtomicU64>`
  the worker thread advances after each `vt_write` batch). Already
  `u64`; just load it.

Each transport's `observe()` populates `WedgeObservation.activity_generation`
from its native primitive. No new shared trait method is added — the
`WedgeProbe` trait's `observe()` signature does not change.

### Decision 3: Busy short-circuit is a pre-classification early return, ordered BEFORE `delivery_ready`

The new branch in `quiescence_classify_step` is structured as:

```
if snapshot_before.activity_generation != snapshot_after.activity_generation {
    state.consecutive_quiescent_mismatches = 0;
    emit_delivery_diagnostic("delivery_target_active", ...);
    return QuiescenceAction::NeedsWait(prime_deadline.unwrap_or_else(unbounded_deadline));
}
```

The branch:

1. Resets the wedge counter (matching the existing
   `operator_interaction_active` branch's reset behavior).
2. Emits the `delivery_target_active` diagnostic so operators can see
   when the short-circuit fired.
3. Returns `NeedsWait` rather than `Done(Ok(...))` — the proposal's
   scope-narrowing applies: while activity is present, the
   classifier does not promote the flush group to ANY terminal
   classification, including `Delivered`. We keep waiting for the
   activity to settle AND for the snapshot to stably match the
   prompt. A post-sleep observation that happens to match the prompt
   regex while bytes are still flowing fires Busy, not Delivered.

**Branch ordering (critical implementation contract).** The Busy
short-circuit MUST be evaluated before any terminal-classification
branch in the same observe-sleep-observe iteration. The full branch
order in `quiescence_classify_step` after the second observation
capture is:

1. `shutdown_requested` check — `Done(Err(Shutdown))`.
2. `operator_interaction_active` check — reset wedge counter,
   emit `delivery_operator_interaction` diagnostic, return
   `NeedsWait`. Operator copy-mode / key-table indefinitely
   suppresses all classification.
3. **Busy short-circuit (NEW).** Activity-generation advance between
   observations → reset wedge counter, emit `delivery_target_active`
   diagnostic with `activity_delta`, return `NeedsWait`. This is the
   new branch from this proposal.
4. `delivery_ready` check — terminal: `snapshot_after.is_prompt_ready &&
   !snapshot_after.operator_interaction_active` →
   `Done(Ok(snapshot_after.pane_target.unwrap_or_default()))`. Emit
   `delivery_ready` diagnostic.
5. Wedge-counter increment block — increments the consecutive-mismatch
   counter when the snapshot is wedge-class AND the activity signal
   was also quiesced (an implicit guard from step 3 returning early).
6. Wedge check — `Done(Err(Wedged))` when the counter reaches
   `WEDGE_CONSECUTIVE_TICKS` or the prime window has elapsed.
7. Prime timeout check — `Done(Err(Timeout))` when the prime window
   has elapsed and the wedge check did not fire.

The Busy short-circuit's placement at step 3 (BEFORE step 4's
`delivery_ready`) is the explicit implementation of the proposal's
"never classify while activity is present" rule. An earlier draft
of this proposal specified the branch as "after operator_interaction
and before the wedge-counter increment block," which would have
left `delivery_ready` (at step 4 in the current code's
`src/transports/quiescence.rs:353-363`, ahead of the
operator-interaction and wedge-counter branches in the original
ordering) ahead of the Busy short-circuit. RG caught that ambiguity
in review; this Decision locks the ordering at "Busy short-circuit
runs BEFORE `delivery_ready`, not just before the wedge-counter
increment block." Operators relying on "if my pane matched the
prompt regex, I got Delivered" semantics should be aware that while
bytes are flowing, the wait continues past a momentary prompt match.

### Decision 4: Tmux fallback to `0` when `#{window_activity}` is unavailable

`resolve_window_activity_marker` already handles the case where the
format is unknown to the running tmux version (returns `Ok(None)` for
`unknown format` / `invalid format` / `bad format` stderr substrings).
When the function returns `None`, `TmuxAsWedgeProbe::observe`
populates `activity_generation` with `0`.

The fallback to `0` means the Busy short-circuit never fires on tmux
versions that don't support `#{window_activity}`, which is the
pre-change behavior for those versions. No regression for older tmux
versions; the new behavior only activates when the format is
available.

### Decision 5: New `delivery_target_active` diagnostic, dedup'd by generation

The diagnostic carries:

- `target_session`
- `pane_target` (when the probe surfaces one)
- `activity_delta` (the magnitude of the activity generation advance)

Dedup is implicit: the Busy branch is only entered when
`activity_generation` ADVANCES, so consecutive iterations of "still
busy, generation advanced by 1 each iteration" emit one diagnostic per
generation advance. Operators looking at the diagnostic stream see one
inscription per generation advance, not one per poll tick.

This matches the existing dedup model for `delivery_prompt_mismatch`,
which dedups by `MismatchSignature` (per the `should_emit_prompt_mismatch`
helper in `src/transports/quiescence.rs`).

### Decision 6: No change to `WedgeProbe` trait signature

The trait's `observe()` method signature is unchanged:
`fn observe(&mut self) -> Result<WedgeObservation, String>;`.

The struct's new field is purely additive — existing `WedgeProbe`
implementations (cross-thread `PtyQuiescenceProbe`, same-thread
`WorkerTerminalProbe`, Tmux `TmuxAsWedgeProbe`) are updated in place
to populate the new field, but no trait method or its signature
changes.

This keeps the door open for other probe implementations (e.g., a
future mock probe for tests) without requiring them to know about
activity generation: a probe that doesn't care about activity can
populate the field with a constant (`0`), and the classifier falls
back to the pre-change behavior for that probe.

## Risks / Trade-offs

- **Risk (Class B silent-thinking case NOT covered)**: A target whose
  agent is actively computing but produces **zero** terminal bytes
  during the entire `quiet_window` interval will populate
  `activity_generation` with a constant value (whatever the last
  observed `#{window_activity}` / `last_change_atomic` was). The
  comparator will never trigger as "advanced", so the Busy
  short-circuit will never fire, and the existing wedge classifier
  will continue to fire `pane_wedged` on the silent-thinking target
  — exactly the same false-positive case the change was supposed to
  prevent. **This is a deliberate scope decision** (Class B is filed
  as a follow-up per the proposal's "Out of scope" section), but it
  deserves to be flagged prominently here so reviewers do not infer
  broader coverage than the change actually provides.
  - **Mitigation**: file a follow-up proposal (`add-wedge-detection-
    process-alive` or similar) that adds a second
    `process_activity_generation: u64` field on `WedgeObservation`,
    populated from child-process CPU time (`/proc/<pid>/stat` on
    Linux, `proc_pidinfo` on macOS, `GetProcessTimes` on Windows);
    the Busy short-circuit then fires when EITHER field advances.
    The follow-up SHOULD land additively on top of this change so
    the Class A fix in this proposal is not blocked on the Class B
    design.

- **Risk (operator's reported incident classification uncertain)**:
  The empirical incident (`issues/relay/46`, 19:31:39.538) is
  described as RG "actively generating a response" — most plausibly
  Class A (bytes flowing but not reflected in the snapshot), but
  Class B (silent pre-output generation) is also possible if RG's
  client buffered its response for the full 2.5s window. We have no
  recorded `#{window_activity}` value at the failure moment to
  disambiguate. The proposal's scope is honest about this: Class A
  is the targeted coverage; Class B is acknowledged as a separate
  bug.

- **Risk**: Tmux `#{window_activity}` is a window-level signal, not a
  pane-level signal. If a tmux session has multiple panes and only one
  is producing activity, the others see "busy" from the wrong pane.
  - **Mitigation**: in practice, each `agentmux`-managed tmux session
    hosts one pane (the coder session); multi-pane tmux sessions are
    not a current use case. If multi-pane support becomes a real
    need, swap to `#{pane_activity}` (a per-pane signal) — the
    change is local to `src/tmux/pane.rs` and the probe wiring.

- **Risk**: a long-running terminal that produces a continuous trickle
  of output (e.g., a `tail -f` on a log file) advances activity
  generation every iteration. The wedge counter is never incremented,
  but the classifier never resolves `Ok(pane)` either (because the
  prompt is never reached). This is a regression only if
  `prime_timeout_ms` is configured: the prime window does fire on a
  truly-busy pane that never settles. With `prime_timeout_ms = None`
  (the Tmux default), the wait continues indefinitely — same as today
  for an unbounded wait.
  - **Mitigation**: this is exactly the operator's intended model
    ("never classify as wedged while activity is present"). The
    prime window is the bounded path; operators who don't want a
    bounded wait should not configure `prime_timeout_ms`.

- **Risk**: Pty's `last_change_atomic` advances AFTER `vt_write`, so
  the Busy short-circuit fires slightly later than the actual write
  occurred. This is a sub-microsecond latency in practice.
  - **Mitigation**: not a real concern; the activity signal is a
    coarse-grained "is the target alive" signal, not a high-
    resolution timing measurement.

- **Trade-off**: the `delivery_target_active` diagnostic is one more
  event in the diagnostic stream. Operators tuning their diagnostic
  filters will need to know about it. Documented in
  `documentation/operations/diagnostic-events.md` (if such a file
  exists) as part of the implementation.

## Migration Plan

No migration needed:

- All existing wedge-classification tests continue to pass without
  changes to the baseline five probe classes. Those probes do not
  advance activity between polls (they advance pane state via the
  scripted probe), so they exercise the `activity_quiesced` path and
  the Busy short-circuit is never entered.
- `WedgeObservation` gains a field; existing construction sites are
  updated to populate it. Any external users of `WedgeObservation`
  (none today — it's an internal cross-transport type) would need to
  update their `Default` derivation. The `#[derive(Default)]`
  continues to work; the new field defaults to `0`.
- No config changes. No new per-coder config knob. The activity
  signal is a transport-internal primitive, not a user-facing knob.

## Open Questions

- **Class B process-alive follow-up: which OS APIs and which
  platforms for v1?** Linux `/proc/<pid>/stat` is reliable and the
  Pty worker can hold the child handle via `Child::process_id()` from
  std; macOS `proc_pidinfo` with `PROC_PIDTASKINFO` is the natural
  equivalent; Windows `GetProcessTimes` is the natural equivalent.
  Proposal scope should commit to cross-Unix for v1 (Linux + macOS)
  and defer Windows per the existing `todos/pty/7` work item, OR
  land cross-platform from the start — call out in the follow-up
  proposal's non-goals.

- **Should `activity_generation` be exposed to the relay or kept
  transport-internal?** Today, `WedgeObservation` is a transport-
  internal type (it doesn't appear in `DeliveryEnvelope` or any
  relay-facing API). Keeping it transport-internal preserves that
  boundary. If operators later want to surface activity state on the
  relay side (e.g., "target is busy, send will queue"), a separate
  proposal would plumb it through the `OutputView` or a new field on
  `TransportStatus`. Not in scope here.

- **Should the Busy short-circuit emit a diagnostic on EVERY iteration
  or only on generation advances?** The current design emits only on
  generation advances (implicit dedup by `!=` comparison). This
  matches the operator's "activity reported" mental model. If
  operators want per-poll-tick visibility into busy state, that's a
  different diagnostic (e.g., `delivery_target_active_poll`) and a
  different decision.