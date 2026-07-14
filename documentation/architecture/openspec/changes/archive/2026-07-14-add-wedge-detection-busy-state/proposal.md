# Change: Wedge detection — positive busy signal short-circuits wedged classification

## Why

`agentmux`'s wedge/prime quiescence classifier (`src/transports/quiescence.rs`,
shared by Tmux and Pty) today uses "did the rendered pane/screen content
change between two consecutive observation polls" as its sole signal that
the target is alive. A target that has terminal output flowing reads as
non-quiescent; a target whose rendered content stops updating reads as
quiescent and (if the prompt-readiness regex also fails to match)
accumulates wedge-counter ticks until the classifier fires
`pane_wedged`.

The defect is a missing positive activity signal. The current `quiescent`
concept conflates "pane content unchanged" with "no activity." Two
distinct activity cases that the classifier cannot currently distinguish
from "frozen pane":

- **Class A — output-write activity not reflected in the captured
  snapshot.** Both transports can produce activity that does not appear in
  `capture_pane_snapshot` (Tmux) or in the rendered `inspected_tail`
  (Pty) at the moment of observation. Examples include terminal escape
  sequences that re-render a status line in place without changing the
  rendered character cells, partial scroll-complete-then-redraw sequences,
  and cursor-blanking sequences that mutate internal state without
  rendering a new character. In these cases, bytes ARE being written to
  the terminal; the snapshot comparison simply fails to detect the
  change. Adding a positive activity signal based on the
  terminal-output-write marker (`#{window_activity}` for Tmux,
  `last_change_atomic` for Pty) catches this case.

- **Class B — silent thinking / pre-output generation.** A target whose
  agent is actively computing (model thinking, multi-step tool-call
  prep, large-context response generation) but is producing zero
  terminal output bytes for the entire `quiet_window` interval. In this
  case neither `#{window_activity}` nor `last_change_atomic` advances,
  because no bytes are being written. This case requires a
  process-level signal (e.g., child-process CPU time via
  `/proc/<pid>/stat` on Linux or `proc_pidinfo` on macOS) which is
  fundamentally different from the existing primitives. **Class B is
  out of scope for this proposal** — see "Out of scope" below.

The operator-confirmed incident in `issues/transports/2` and
`issues/relay/46` (RG's session fired `pane_wedged` at 19:31:39.538
during a 2.476s send, while RG was "actively generating a response")
is most plausibly Class A — "actively generating" strongly implies
bytes being produced, which would advance the markers even if the
snapshot comparison doesn't reflect them. We have no recorded
`#{window_activity}` value at the failure moment to confirm, but the
description is consistent with Class A. Class B is plausible only if
RG's client had been buffering its response for the full 2.5s, which
is an unusual but possible agent behavior.

## Scope clarification

This proposal covers **Class A** only. The proposed signal —

- **Tmux**: `#{window_activity}` (the project's existing
  window-level terminal-output-write marker, advanced by tmux on any
  pane write event).
- **Pty**: `last_change_atomic` on `PtyShared` (the worker thread's
  per-target generation counter, advanced after each `vt_write`
  batch).

— is a **terminal-output-write** marker, not a process-busy marker.
The marker advances when bytes are written to the terminal; it does
NOT advance when the agent is thinking with zero byte output. This
distinction is the explicit scope of this proposal. Class B
(silent-thinking / pre-output) is a real bug class but requires
different primitives; it is filed as a separate follow-up proposal
(see "Out of scope" below).

## What Changes

- Add an `activity_generation: u64` field to `WedgeObservation` in
  `src/transports/quiescence.rs`. The field carries a per-transport
  monotonic **terminal-output-write** marker captured at observation
  time; the classifier uses it as the positive "output is flowing"
  signal. **The field is a terminal-output-write marker, NOT a
  process-busy marker.** See "Scope clarification" above and
  "Out of scope" below for what this proposal does and does not
  cover.
- Each transport's `observe()` populates the field from a transport-
  native primitive:
  - **Tmux**: `resolve_window_activity_marker` in `src/tmux/pane.rs`
    (the existing `#{window_activity}` query used today by
    `RealPaneQuiescenceProbe::wait_for_change`). Parse the marker as a
    `u64` epoch-seconds value; fall back to `0` when the format is
    unavailable on the running tmux version (the existing function
    already returns `Ok(None)` for `unknown format`).
  - **Pty**: `last_change_atomic` on `PtyShared` in `src/pty/state.rs`
    (the existing `Arc<AtomicU64>` the worker advances after each
    `vt_write` batch). Already `u64`; load with `Ordering::Acquire`.
- Add a new pre-classification branch in
  `quiescence_classify_step`: when the activity generation advanced
  between two consecutive observations, treat the target as **Busy**:
  reset the wedge counter to 0, do NOT inspect `is_prompt_ready`, and
  return `QuiescenceAction::NeedsWait(...)` without firing any terminal
  classification. This implements the "never classify as wedged while
  activity is present" rule from the operator's model.

  **Branch ordering (critical).** The Busy short-circuit MUST be
  evaluated BEFORE any terminal-classification branch in the same
  observe-sleep-observe iteration. In `quiescence_classify_step`, the
  branch order after the second observation capture is:

  1. `operator_interaction_active` check — reset counter, return
     `NeedsWait` (operator copy-mode/key-table indefinitely
     suppresses all classification).
  2. **Busy short-circuit (new)** — reset counter, emit
     `delivery_target_active` diagnostic, return `NeedsWait`.
  3. `delivery_ready` check — terminal: return
     `Done(Ok(pane_target))`.
  4. Wedge-counter increment block.
  5. Wedge check → `Done(Err(Wedged))`.
  6. Prime timeout check → `Done(Err(Timeout))`.

  The Busy short-circuit's placement at step 2 (before step 3's
  `delivery_ready`) is what implements "while activity is present,
  the classifier does not promote the flush group to any terminal
  classification" — including `Delivered`. A post-sleep observation
  that happens to match the prompt regex while the agent is still
  producing output bytes fires Busy (return `NeedsWait`) rather
  than `Delivered`, and the wait continues until activity settles
  AND the snapshot stably matches the prompt.
- The existing wedge counter logic (increment on `quiescent &&
  !is_prompt_ready && wedge_class`; fire when the counter reaches
  `WEDGE_CONSECUTIVE_TICKS` or the prime window elapses) is preserved
  but is now implicitly guarded by the Busy short-circuit: the counter
  only advances in iterations where the activity generation was also
  quiesced.
- Emit a new `delivery_target_active` diagnostic inscription when the
  Busy short-circuit fires, with `target_session`, `pane_target`, and
  `activity_delta` (the magnitude of the activity-generation advance).
  The inscription is dedup'd against the current activity generation so
  a continuously-busy target emits one inscription per generation
  advance, not per poll tick. Operators can grep the diagnostic stream
  to distinguish "busy short-circuit fired" from "wedge classification
  fired".
- Modify the `Three-State Delivery Classifier` requirement in
  `openspec/specs/transport-abstraction/spec.md` to add the Busy
  short-circuit and the related scenarios. Add a new `Positive Activity
  Signal` requirement specifying the per-transport activity source.
- Existing tests continue to pass without changes to the baseline five
  probe classes (they already use scripted `wait_for_change` and
  do not advance activity between polls, so they exercise the
  `activity_quiesced` path). Add new tests covering the Busy short-
  circuit:
  - Tmux: a probe whose `window_activity` advances between observations
    but whose pane content does not changes — assert no wedge fires
    even with `WEDGE_CONSECUTIVE_TICKS = 3` quiesced-tick budget.
  - Pty: a probe whose `last_change_atomic` advances between observations
    but whose `inspected_tail` does not — same assertion.
  - Cross-cutting: a probe that alternates activity-on / activity-off
    — assert no wedge ever fires.
- No breaking changes to the `WedgeProbe` trait signature; the
  `observe()` method's return type gains the `activity_generation`
  field but does not change its `Result<WedgeObservation, String>`
  shape. Existing implementations of `WedgeProbe` (the cross-thread
  `PtyQuiescenceProbe`, the same-thread `WorkerTerminalProbe`, and the
  Tmux `TmuxAsWedgeProbe` adapter) are updated in place.

## Impact

- Affected specs: `transport-abstraction` (the
  `Three-State Delivery Classifier` requirement is modified; a new
  `Positive Activity Signal` requirement is added).
- Affected code:
  - `src/transports/quiescence.rs` — add `activity_generation` field to
    `WedgeObservation`; add the Busy short-circuit branch in
    `quiescence_classify_step`; emit `delivery_target_active` diagnostic.
  - `src/transports/mod.rs` — no change (re-exports already cover the
    public surface).
  - `src/tmux/quiescence_probe.rs` — `TmuxAsWedgeProbe::observe` reads
    `resolve_window_activity_marker` (or its equivalent) and populates
    the new field. The existing `RealPaneQuiescenceProbe::wait_for_change`
    activity-marker polling is unchanged.
  - `src/pty/state.rs` — `PtyQuiescenceProbe::observe` and
    `WorkerTerminalProbe::observe` read `last_change_atomic.load(...)`
    and populate the new field.
  - `tests/unit/tmux_transport.rs` — extend the scripted probe surface
    with an activity-advance sequence; add Busy short-circuit assertions.
  - `tests/unit/pty_transport.rs` — extend the `make_term_protocol_transport`
    helper (or analogous helper) with an activity-advance sequence; add
    Busy short-circuit assertions.

## Non-goals

- **Generic wedge-classification redesign.** This change adds the
  missing positive busy signal; it does not redesign the wedge-class
  taxonomy (e.g., introduce additional classifications between Busy
  and Wedged) or change the `WEDGE_CONSECUTIVE_TICKS` threshold.
- **Replacing `#{window_activity}` with a more sensitive signal.**
  Tmux's `#{window_activity}` is the project's existing activity
  primitive; reusing it keeps the design within the bounds of what
  tmux makes observable. A future change could swap in a finer-
  grained signal (e.g., `#{pane_activity}` for per-pane tracking) if
  empirical data shows the window-level signal is too coarse.
- **Changing `WEDGE_CONSECUTIVE_TICKS` or the prime window.** This
  change does not retune the timing constants. The Busy short-circuit
  makes the timing constants matter less (the counter only advances
  during genuinely-quiesced periods), so any retuning is deferred
  until empirical data on the new behavior is available.
- **Per-pane activity tracking for Pty.** Pty uses
  `last_change_atomic` which is per-target (one Pty transport =
  one child process = one atomic). Per-pane tracking is a Tmux-only
  consideration; Pty's atomic is already per-pane by construction.
- **Sibling `issues/transports/1` (quiescence-timeout outcome
  mis-report).** That issue is a different bug in the same code area
  (a delivery that exhausts the quiescence timeout without ever
  reaching `delivery_ready` is reported as `delivered` instead of
  `failed` with `reason_code = "quiescence_timeout"`). Out of scope
  for this proposal; tracked separately.

## Out of scope (deferred to follow-up proposals)

- **Class B (silent thinking / pre-output generation).** A separate
  proposal (NOT this one) introduces a process-level aliveness signal
  that catches the case where the agent produces zero terminal bytes
  during `quiet_window`. The proposed signal source is child-process
  CPU time, polled via `/proc/<pid>/stat` on Linux, `proc_pidinfo`
  with `PROC_PIDTASKINFO` on macOS, and `GetProcessTimes` on Windows.
  This is non-trivial: per-platform OS API plumbing, careful handling
  of zombie / recently-started processes, and Windows-host gating
  (cross-Unix for v1, Windows deferred per the existing
  `todos/pty/7` work item). Filed against `issues/transports/2` in a
  follow-up note; tracked as a separate proposal at the time the
  Class A fix lands and stabilizes. The Class B proposal SHOULD add
  the process-level signal as a SECOND `activity_generation`-style
  field on `WedgeObservation` (e.g.,
  `process_activity_generation: u64` populated from CPU time), and
  the classifier's short-circuit SHOULD fire when EITHER field
  advances between observations. This keeps the merge shape additive
  on top of the Class A signal landed here.

- **Empirical tuning of `WEDGE_CONSECUTIVE_TICKS` and the prime
  window** once the output-activity short-circuit is in production and
  we have data on how often the counter actually advances in the wild.
- **A finer-grained activity signal for Tmux** (`#{pane_activity}`
  per-pane tracking) if window-level granularity proves too coarse.
- **`issues/transports/1` fix** — quiescence-timeout outcome mis-report;
  same code neighborhood but different bug.
- **Diagnostic-trace enrichment** — additional `delivery_target_active`
  metadata (e.g., `busy_for_ms` measured from `prime_started_at`) if
  operators need it for incident diagnosis.

## Validation plan

- `cargo nextest run --locked --config-file
  .auxiliary/configuration/nextest.toml` passes with no regressions on
  the existing wedge-classification tests (the baseline five probe
  classes).
- `cargo nextest run --locked --config-file
  .auxiliary/configuration/nextest.toml --features pty --run-ignored
  all -E 'test(/busy|activity/)'` passes — covers the new Busy short-
  circuit scenarios under the Pty feature.
- `cargo clippy --all-targets --no-deps -- -D warnings` silent.
- `cargo clippy --all-targets --features pty --no-deps -- -D warnings`
  silent.
- `cargo fmt --check` silent.
- `openspec validate add-wedge-detection-busy-state --strict` valid.
- Manual: a real bundle run where a target session is mid-generation
  when a `send` arrives, observed via `agentmux look <session>`,
  produces a `delivery_target_active` inscription in the diagnostic
  stream and resolves the flush group as `Delivered` (or waits for
  the prompt normally) — never as `Failed + reason_code = "pane_wedged"`.
  This is the same shape of live-validation as the
  `add-pty-terminal-protocol-config` `tasks.md` §6.6 joint session;
  owner and timing TBD per the lane workflow.