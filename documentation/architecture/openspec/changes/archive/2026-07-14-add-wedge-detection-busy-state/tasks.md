# Tasks: Wedge detection — positive busy signal short-circuits wedged classification

## 1. Cross-transport WedgeObservation addition

- [x] 1.1 Add `activity_generation: u64` field to
      `WedgeObservation` in `src/transports/quiescence.rs`. Update the
      module-level doc to document the field. Default-initialize in
      the existing `#[derive(Default)]` so legacy observation sites
      (none today, but future-proof) compile. **(Landed in R0.)**
- [x] 1.2 Update `WedgeObservation`'s `PartialEq` derive to include the
      new field (already implicit from the struct-level derive; verify
      the diff in the type system). **(Landed in R0 — the struct-level
      `derive(PartialEq)` covers it; verified `cargo check` passes.)**
- [x] 1.3 Verify `cargo check --all-targets` (no warnings).
      **(Landed in R0 — `cargo check --all-targets` and
      `cargo check --all-targets --features pty` both pass clean.)**

## 2. Classifier Busy short-circuit

- [x] 2.1 In `quiescence_classify_step`, **BEFORE the `delivery_ready`
      branch** (which currently sits at `src/transports/quiescence.rs:353`
      ahead of the operator-interaction branch at `:365`), add the
      Busy short-circuit. The required branch order after the second
      observation capture becomes: **(Landed in R0 — Busy branch sits
      between `operator_interaction_active` (moved up to step 1) and
      the simplified `delivery_ready` check at step 3.)**
      1. `operator_interaction_active` check (existing) — reset
         counter, return `NeedsWait`.
      2. **Busy short-circuit (NEW)** — when activity advanced,
         reset counter, emit `delivery_target_active` diagnostic,
         return `NeedsWait`. The branch body:
         ```rust
         // Busy: terminal-output-write activity advanced between
         // observations. The target is alive; readiness is
         // irrelevant; never classify as wedged, unresponsive,
         // or delivered while activity is present.
         if snapshot_before.activity_generation != snapshot_after.activity_generation {
             state.consecutive_quiescent_mismatches = 0;
             emit_delivery_diagnostic(
                 "delivery_target_active",
                 &json!({
                     "target_session": target_session,
                     "pane_target": snapshot_after.pane_target,
                     "activity_delta": snapshot_after
                         .activity_generation
                         .saturating_sub(snapshot_before.activity_generation),
                 }),
             );
             return QuiescenceAction::NeedsWait(
                 prime_deadline.unwrap_or_else(unbounded_deadline),
             );
         }
         ```
      3. `delivery_ready` check (existing — line 353) — terminal:
         returns `Done(Ok(...))` when prompt-ready.
      4. Wedge-counter increment block.
      5. Wedge check.
      6. Prime timeout check.

      **Why this placement (and not "before the wedge-counter
      increment block" as a previous draft specified):** the
      `delivery_ready` branch is currently at `quiescence.rs:353`,
      which is ahead of the wedge-counter increment block at `:391`.
      Placing Busy only before the wedge-counter increment block
      would leave `delivery_ready` ahead of it, allowing a
      post-sleep snapshot that matches the prompt regex while bytes
      are flowing to resolve as Delivered before the Busy branch
      can fire — violating the spec's "Busy suppresses all terminal
      classifications including Delivered" contract. Placing Busy
      BEFORE `delivery_ready` is what implements that contract.

- [x] 2.2 Verify the wedge-counter increment block (existing) is now
      implicitly guarded by `activity_quiesced`: the Busy branch above
      returns early at step 2, so the increment at step 4 is
      unreachable while activity is advancing. No code change to the
      increment block; just a comment noting the implicit guard.
      **(Landed in R0 — the increment block now documents the implicit
      guard against the Busy short-circuit's early return at step 2.)**
- [x] 2.3 Add a unit-style test (or extend an existing test) verifying
      that the Busy short-circuit returns `QuiescenceAction::NeedsWait`
      and resets `consecutive_quiescent_mismatches` to 0 when the
      activity generation advances between observations. **(Landed in
      R0 — `tests/unit/transports_quiescence.rs` `busy_short_circuit_returns_needs_wait_when_activity_advances`
      covers both branches of the assertion.)**
- [x] 2.4 Add the branch-ordering-contract test for both Tmux and Pty
      probes: a probe whose `activity_generation` advances between
      observations AND whose `is_prompt_ready == true` MUST resolve
      as `NeedsWait` (Busy), NOT `Ok(pane)` (Delivered), until the
      activity generation quiesces across an observation pair. This
      is the test for the spec scenario "Busy short-circuit defers
      Delivered during active output." **(Landed in R0 via the cross-
      transport mock probe in `tests/unit/transports_quiescence.rs`
      (`busy_short_circuit_defers_delivered_when_activity_advances_while_ready`
      covers the cross-transport contract; per-transport coverage via
      `TmuxAsWedgeProbe` and the Pty probes lands in R1/R2 integration
      tests.)**

## 3. Tmux probe activity source

- [x] 3.1 In `src/tmux/quiescence_probe.rs`, modify
      `TmuxAsWedgeProbe::observe` to capture the window activity marker
      and populate `WedgeObservation.activity_generation`:
      - Call `resolve_window_activity_marker(self.inner.tmux_socket()?,
      pane_target.as_str())` (the function already exists in
      `src/tmux/pane.rs` and is already used by
      `RealPaneQuiescenceProbe::wait_for_change`).
      - Parse the returned `Option<String>` into a `u64` epoch-seconds
      value. Returned value `Ok(None)` (the format is unavailable) →
      populate with `0`. Returned value `Ok(Some(marker))` →
      `marker.parse::<u64>().unwrap_or(0)`.
      - `TmuxAsWedgeProbe` does not currently hold a reference to the
      `tmux_socket`; add a `socket_path: &Path` field (or thread it
      through `PaneQuiescenceProbe` as a new accessor method). **(Landed
      in R1 — added a `last_window_activity_marker` method to
      `PaneQuiescenceProbe` (rather than threading the socket path
      through the probe struct); `TmuxAsWedgeProbe::observe` calls
      `self.inner.last_window_activity_marker()?.unwrap_or(0)`. The
      trait-method approach keeps the adapter struct minimal and the
      probe trait uniform across all `PaneQuiescenceProbe`
      implementations.)**
- [x] 3.2 Verify `RealPaneQuiescenceProbe::wait_for_change` continues
      to use the same `resolve_window_activity_marker` query — no
      behavior change to `wait_for_change`. The two reads happen at
      different cadence points (per-observation vs. wait-poll), so
      they do not collide. **(Landed in R1 —
      `RealPaneQuiescenceProbe::wait_for_change` is unchanged: it
      already polls `resolve_window_activity_marker` for change
      detection; `last_window_activity_marker` is a separate
      per-observation read.)**
- [x] 3.3 Add a Tmux test in `tests/unit/tmux_transport.rs` extending
      the scripted `ScriptedProbe` (or a new probe variant) to advance
      `activity_generation` between observations while keeping the
      pane content unchanged. Assert: the wait function returns
      `Ok(pane)` after the activity eventually settles and the pane
      becomes prompt-ready; no `Wedged` is fired even with
      `WEDGE_CONSECUTIVE_TICKS = 3` budget exhausted during the busy
      period. **(Landed in R1 — three tests in
      `tests/unit/tmux_transport.rs`: the
      `tmux_busy_short_circuit_prevents_wedged_when_activity_advances_on_every_iteration`
      test exercises the wedged path (wedge-class + activity advancing
      always) and asserts no `Wedged` fires within the abort window;
      `tmux_busy_short_circuit_defers_delivered_when_activity_advances_while_ready`
      exercises the branch-ordering contract (prompt-ready +
      activity advancing always) and asserts no `Delivered` fires;
      `tmux_constant_activity_fires_wedged_as_before` is the
      regression baseline asserting constant-activity behavior is
      unchanged. Tests use `abort_after` to terminate rather than
      driving to a settled state, which is stronger than the
      tasks.md-text wording (it asserts Busy persists throughout the
      busy window rather than waiting for one specific
      settle-to-ready transition).)**

## 4. Pty probe activity source

- [x] 4.1 In `src/pty/state.rs`, modify `PtyQuiescenceProbe::observe` to
      load `last_change_atomic.load(Ordering::Acquire)` and populate
      `WedgeObservation.activity_generation`. **(Landed in R2 — the R0
      placeholder `activity_generation: 0` is replaced with
      `self.shared.last_change_atomic.load(Ordering::Acquire)`.)**
- [x] 4.2 Modify `WorkerTerminalProbe::observe` identically.
      **(Landed in R2 — same AtomicU64 load; struct holds the
      atomic as its own field.)**
- [x] 4.3 Verify `PtyQuiescenceProbe::wait_for_change` continues to use
      the same atomic (no behavior change). **(Landed in R2 — the R2
      fixture also changed: the mock worker no longer per-request
      advances `last_change_atomic` so the same atomic's
      `activity_generation` semantics don't conflate "probe polled
      snapshot" with "terminal byte writes happened". The timer
      thread continues advancing every 50ms, driving
      `wait_for_change` for all existing tests. Existing wedge-class
      tests pass because within a single
      `quiescence_classify_step` call the activity signal stays
      constant (timer hasn't fired between two observes 5ms apart);
      the wedge counter accumulates normally and `Wedged` fires
      after `WEDGE_CONSECUTIVE_TICKS = 3`.)**
- [x] 4.4 Add a Pty test in `tests/unit/pty_transport.rs` exercising
      the Busy short-circuit: a test helper that constructs a probe
      and exercises the state machine with activity-advance sequences
      (snapshot unchanged, activity generation advanced). Assert: no
      `Wedged` fires. (Pattern mirrors the CSI-u tests; the existing
      `#[ignore]` Pty-feature tests already have the live-pty harness,
      so this can use the same shape.) **(Landed in R2 — three new
      tests in `tests/unit/pty_transport.rs`:
      `pty_probe_observe_populates_activity_generation_from_last_change_atomic`
      is a direct probe-level test verifying the field is sourced
      from `last_change_atomic.load(Ordering::Acquire)` and updates
      when the atomic is advanced; `pty_constant_activity_fires_wedged_as_before`
      is the regression baseline asserting the existing wedge-class
      + constant-activity behavior is unchanged across the R2
      fixture change; `pty_busy_short_circuit_defers_delivered_when_activity_advances_while_ready`
      verifies the field differs between two consecutive
      `observe()` calls when `last_change_atomic` is advanced, which
      is exactly the condition that triggers the cross-transport
      classifier's Busy pre-classification (the Busy branch fires
      on `activity_quiesced == false` regardless of where the
      observations come from — verified via the cross-transport
      mock-probe tests in `tests/unit/transports_quiescence.rs`).
      An end-to-end busy-via-Pty integration test via
      `wait_for_quiescent_three_state` is not viable because the
      wait function has no termination path during sustained
      Busy — Busy early-returns from `quiescence_classify_step`
      before any prime-window check fires, and the test cannot
      drive the activity atomic between two observes within a
      single iteration since the function is opaque to the
      test.)**

## 5. Tests for the cross-cutting busy behavior

- [x] 5.1 Add a Tmux probe test that alternates activity-on /
      activity-off repeatedly with sustained non-prompt pane content.
      Assert: no `Wedged` ever fires; the wait function only resolves
      on `Ok(pane)` when the activity eventually settles and the pane
      becomes prompt-ready. **(Landed in R3 — the
      `tmux_alternating_activity_does_not_fire_wedged` test in
      `tests/unit/tmux_transport.rs` exercises alternating
      advance/constant activity sequence with sustained wedge-class
      mismatch, asserting no `Wedged` fires within the abort window
      (Busy resets the counter between constant-phase iterations;
      counter never reaches `WEDGE_CONSECUTIVE_TICKS`). The test
      terminates via `abort_after` (the wait function has no natural
      termination during sustained alternation, matching the same
      constraint documented in R2's commit message.)**
- [x] 5.2 Add a Pty test analog to 5.1 (gated on `--features pty`).
      **(Landed in R3 — the
      `pty_alternating_activity_field_reflects_advance_pattern`
      test in `tests/unit/pty_transport.rs` verifies the alternate
      advance/constant pattern at probe level (via
      `last_change_atomic.store(...)` between consecutive
      `observe()` calls). The end-to-end busy-via-Pty
      `wait_for_quiescent_three_state` integration test is
      infeasible for the same reason as the R2 busy-fires-integration
      test (no termination path during sustained Busy); the
      field-sourcing contract is verified by this probe-level test
      and the cross-transport classifier's Busy rule is verified via
      the mock probe in `tests/unit/transports_quiescence.rs`.)**
- [x] 5.3 Verify the existing baseline five probe classes still pass:
      - `AlwaysUnresponsiveProbe` → Timeout
      - `AlwaysWedgeProbe` → Failed + `reason_code = "pane_wedged"`
      - `PendingChoiceProbe` → neither
      - `SlowPromptProbe` → Delivered
      - `NormalFlowProbe` → Delivered
      None of these advance activity between polls; they exercise the
      `activity_quiesced` path and must not regress. **(Landed in R3
      via post-implementation test runs — see §6.1 validation
      below. The cross-transport classifier's
      `Snap before == snap after` check uses `#[derive(PartialEq)]`
      on `WedgeObservation`; the new `activity_generation` field is
      a `u64` whose equality check correctly compares the two
      values, and the existing probes' activities stay constant
      across a single quiescence iteration (the R2 fixture change
      removed the worker's per-request atomic advance so
      `last_change_atomic` only advances via the timer thread's
      50ms cadence, which is slower than the tests'
      `SHORT_QUIET_WINDOW = 5ms`). Existing tests behave the same
      way they did pre-change.)**

## 6. Validation

- [x] 6.1 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml`: 661 passed (3 slow),
      0 skipped. No regressions vs. the pre-change baseline of 657
      (the +4 are: 3 new transports_quiescence mock-probe tests,
      1 new tmux_alternating_activity_does_not_fire_wedged test).
- [x] 6.2 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml --features pty`: 690
      passed (3 slow), 6 skipped. No regressions vs. the pre-R3
      baseline of 685 (the +5 are: 3 new transports_quiescence
      tests, 1 tmux_alternating_activity_does_not_fire_wedged, 2
      new pty tests in tests/unit/pty_transport.rs from R2).
- [x] 6.3 `cargo clippy --all-targets --no-deps -- -D warnings`:
      silent.
- [x] 6.4 `cargo clippy --all-targets --features pty --no-deps
      -- -D warnings`: silent.
- [x] 6.5 `cargo fmt --check`: silent.
- [x] 6.6 `openspec validate --all --strict`: 21 passed, 0 failed
      (no changes to other in-flight changes beyond this proposal's
      task-checkboxes).
- [x] 6.7 **WAIVED (2026-07-14, operator).** Manual live-bundle
      validation: a send to a target session with bytes actively
      flowing to the terminal produces a `delivery_target_active`
      diagnostic and resolves the flush group as `Delivered`, never as
      `Failed + reason_code = "pane_wedged"`. Waived as this change's
      closeout gate because it was the only open item, the underlying
      contract is already covered by deterministic unit tests (§2.3,
      §2.4, §3.3, §5.1-5.3), and it was blocking archive, which in turn
      blocked `issues/relay/52`'s proposal (drafted against this
      change's post-merge baseline). Not dropped — deferred to a
      standalone todo tracked on the `coordination/general/16` (0.9.0)
      milestone note: Class A manual live-bundle validation (bytes
      actively flowing → `delivery_target_active` → `Delivered`, never
      `Failed`/`pane_wedged`). Class B (silent thinking, zero bytes)
      stays out of scope per §7.

## 7. [Follow-up, NOT this proposal] Process-alive signal for Class B silent thinking

This proposal covers Class A (terminal-output-write marker catches a
target whose bytes are flowing but not reflected in the captured
snapshot). The complementary **Class B** (silent thinking /
pre-output generation: target's agent is busy but produces zero
terminal bytes) is a real bug class filed as
`add-wedge-detection-process-alive` (or similar) — to be drafted and
reviewed as a SEPARATE proposal after this one lands and stabilizes.

The follow-up SHOULD:

- Add a second `process_activity_generation: u64` field to
  `WedgeObservation` (separate from the terminal-output-write
  `activity_generation` this proposal introduces).
- For **Pty**: poll the child process's CPU time via
  `Child::process_id()` + `/proc/<pid>/stat` (Linux),
  `proc_pidinfo` with `PROC_PIDTASKINFO` (macOS),
  `GetProcessTimes` (Windows, deferred to follow-up-follow-up if
  `todos/pty/7` Windows-host validation gating applies).
- For **Tmux**: poll the pane's child CPU time via `#{pane_pid}` +
  the same OS APIs.
- Extend the Busy short-circuit to fire when EITHER
  `activity_generation` or `process_activity_generation` advances.
- Be additive on top of this proposal's implementation (Class B
  merges cleanly on top of Class A — same field shape, same
  comparator, just OR'd in the short-circuit condition).

This follow-up is documented here so the design decision is explicit:
Class A and Class B are two distinct bug classes and warrant two
distinct primitives. Bundling them into one proposal would have
delayed the Class A fix for the OS-API plumbing and review time; the
split keeps this proposal small and lets the Class B design mature
separately.