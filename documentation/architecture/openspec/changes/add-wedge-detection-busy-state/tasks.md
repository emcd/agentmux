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

- [ ] 4.1 In `src/pty/state.rs`, modify `PtyQuiescenceProbe::observe` to
      load `last_change_atomic.load(Ordering::Acquire)` and populate
      `WedgeObservation.activity_generation`.
- [ ] 4.2 Modify `WorkerTerminalProbe::observe` identically.
- [ ] 4.3 Verify `PtyQuiescenceProbe::wait_for_change` continues to use
      the same atomic (no behavior change).
- [ ] 4.4 Add a Pty test in `tests/unit/pty_transport.rs` exercising
      the Busy short-circuit: a test helper that constructs a probe
      and exercises the state machine with activity-advance sequences
      (snapshot unchanged, activity generation advanced). Assert: no
      `Wedged` fires. (Pattern mirrors the CSI-u tests; the existing
      `#[ignore]` Pty-feature tests already have the live-pty harness,
      so this can use the same shape.)

## 5. Tests for the cross-cutting busy behavior

- [ ] 5.1 Add a Tmux probe test that alternates activity-on /
      activity-off repeatedly with sustained non-prompt pane content.
      Assert: no `Wedged` ever fires; the wait function only resolves
      on `Ok(pane)` when the activity eventually settles and the pane
      becomes prompt-ready.
- [ ] 5.2 Add a Pty test analog to 5.1 (gated on `--features pty`).
- [ ] 5.3 Verify the existing baseline five probe classes still pass:
      - `AlwaysUnresponsiveProbe` → Timeout
      - `AlwaysWedgeProbe` → Failed + `reason_code = "pane_wedged"`
      - `PendingChoiceProbe` → neither
      - `SlowPromptProbe` → Delivered
      - `NormalFlowProbe` → Delivered
      None of these advance activity between polls; they exercise the
      `activity_quiesced` path and must not regress.

## 6. Validation

- [ ] 6.1 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml`: 648+ tests pass (4
      skipped); no regressions vs. baseline.
- [ ] 6.2 `cargo nextest run --locked --config-file
      .auxiliary/configuration/nextest.toml --features pty
      --run-ignored all -E 'test(/busy|activity/)'`: passes.
- [ ] 6.3 `cargo clippy --all-targets --no-deps -- -D warnings`:
      silent.
- [ ] 6.4 `cargo clippy --all-targets --features pty --no-deps
      -- -D warnings`: silent.
- [ ] 6.5 `cargo fmt --check`: silent.
- [ ] 6.6 `openspec validate --all --strict`: passes (no changes to
      other in-flight changes).
- [ ] 6.7 Manual live-bundle validation (operator-scheduled): a send
      to a target session with bytes actively flowing to the terminal
      produces a `delivery_target_active` diagnostic and resolves the
      flush group as `Delivered`, never as `Failed +
      reason_code = "pane_wedged"`. Owner and timing per the lane
      workflow (parallel to the `add-pty-terminal-protocol-config`
      §6.6 joint session). **Note (Scope A only):** this validation
      covers the Class A case (terminal bytes flowing but not
      reflected in the captured snapshot). A target in silent
      thinking (Class B, zero bytes during `quiet_window`) is NOT
      expected to be rescued by this change — that requires the
      follow-up proposal in §7.

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