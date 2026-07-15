# Tasks: Pty transport (libghostty-vt, in parallel with Tmux)

## 1. Dependency and module skeleton

- [x] 1.1 Add a default-off `pty` Cargo feature to workspace
      `Cargo.toml`:
      ```toml
      [dependencies]
      libghostty-vt = { version = "=0.2.0", default-features = false,
                        optional = true }
      portable-pty = { version = "0.9.0", optional = true }

      [features]
      default = []
      pty = ["dep:libghostty-vt", "dep:portable-pty"]
      ```
      The `=0.2.0` exact pin for libghostty-vt matches the
      "pin tightly" Decision (pre-1.0 upstream with expected
      breaking changes). `portable-pty = "0.9.0"` is caret-
      compatible (wezterm-maintained, more stable). The default
      `cargo build` does NOT pull in libghostty-vt or portable-pty
      and does NOT invoke Zig. Opting in (`cargo build --features
      pty`) pulls both deps and runs the Zig vendored build.
- [x] 1.2 Add the new bin target `agentmux-pty` at
      `src/bin/agentmux_pty.rs`, gated on the `pty` feature:
      ```toml
      [[bin]]
      name = "agentmux-pty"
      path = "src/bin/agentmux_pty.rs"
      required-features = ["pty"]
      ```
- [x] 1.3 Verify `cargo build` succeeds in default configuration
      (no Zig, no libghostty-vt) and `cargo build --features pty`
      succeeds with the Zig vendored build linked. Validates the
      feature-gate contract end-to-end.
- [x] 1.4 Add `#[cfg(feature = "pty")] pub mod pty;` to `src/lib.rs`.
- [x] 1.5 Create `src/pty/mod.rs` re-exporting `transport`,
      `state`, and the `PtyQuiescenceProbe` adapter. The
      `src/pty/` module is compiled only when the `pty` feature
      is enabled.
- [x] 1.6 Create empty `src/pty/transport.rs`, `src/pty/state.rs`
      with module-level doc comments describing their roles. The
      shared wedge/prime state machine is NOT in this module
      (see §2 — it lives in `src/transports/quiescence.rs`).

## 2. Generalized wedge/prime state machine (cross-transport)

- [x] 2.1 Lift the three-state classifier from
      `src/tmux/transport.rs::wait_for_quiescent_pane_three_state`
      into a new `src/transports/quiescence.rs`, generalized over
      a `WedgeProbe` trait. This module is compiled unconditionally
      (the Tmux transport always imports it; Pty imports it only
      when the `pty` feature is on).
- [x] 2.2 Define `WedgeProbe` in `src/transports/quiescence.rs`:
      a single `observe()` method returning a `WedgeObservation`
      snapshot (inspected_tail, cursor_idle_at, is_prompt_ready,
      operator_interaction_active) plus a `wait_for_change(deadline)`
      method that bounds the wait by the prime deadline and advances
      the probe's state for the next iteration. `is_prompt_ready`
      defaults to `true` for the operator_interaction_active=false
      case in Pty (see deviation note below).
- [x] 2.3 The `DeliveryWaitError::Timeout` and
      `DeliveryWaitError::Wedged` enums stay in
      `src/transports/contract.rs` (already in the right home);
      the new state machine returns them unchanged. `Wedged`
      carries a `reason: String` payload sourced from the probe's
      inspected_tail mismatch derivation.
- [x] 2.4 Add `WEDGE_CONSECUTIVE_TICKS = 3` constant in
      `src/transports/quiescence.rs` (lifted from
      `src/tmux/transport.rs`).
- [x] 2.5 Add `mismatch_is_wedge_class(reason: &str) -> bool`
      helper in `src/transports/quiescence.rs`; same body as today's
      `src/tmux/transport.rs::mismatch_is_wedge_class`.
- [x] 2.6 Refactor `src/tmux/transport.rs::wait_for_quiescent_pane_three_state`
      to construct a small `TmuxAsWedgeProbe` adapter from the
      existing `PaneQuiescenceProbe` and delegate the state
      machine to `src/transports/quiescence.rs`. Preserve the
      existing 16-probe test surface untouched.
- [x] 2.7 Re-export `WedgeProbe` and the lifted state machine from
      `src/transports/mod.rs` alongside `contract`, `ui`, and
      `vocabulary`.

> **Deviation note (tasks.md §2.2):** the trait shape is two methods
> (`observe` + `wait_for_change`) returning a `WedgeObservation`
> snapshot, not the four-method shape listed in tasks.md §2.2
> (`inspect_tail`, `cursor_idle_at`, `is_settled`,
> `operator_interaction_active`). The single-snapshot shape avoids the
> cache-invalidation problem that arises when the state machine calls
> four separate trait methods per observation: a transport whose probe
> side-effects on each method call would otherwise do 4-8x more work
> per iteration than necessary, and the legacy `tmux_transport` test
> probes' `abort_after_calls` counters would trip prematurely. The
> 16-probe test surface in `tests/unit/tmux_transport.rs` is preserved
> unchanged — `TmuxAsWedgeProbe::observe()` calls
> `PaneQuiescenceProbe::next_evaluation()` exactly once per call,
> matching the legacy two-calls-per-iteration cadence.

## 3. Pty transport state and shared view

- [x] 3.1 Define `PtyState` in `src/pty/state.rs`: holds a
      `libghostty_vt::Terminal<'static, 'static>`,
      `libghostty_vt::RenderState<'static>`, the
      `[coders.<id>.pty]` config snapshot (cols, rows, prompt
      template, prime_timeout_ms, wedge_detection), and the
      shared-effect-handler state (`Cell<RefCell<Vec<u8>>>` for
      `on_pty_write` responses).
- [x] 3.2 Define `PtyOutputView` in `src/pty/state.rs` wrapping
      `Arc<Mutex<PtyState>>`. Implement the `OutputView` trait:
      - `look(mode)` — locks the mutex, recreates the formatter
        from `&terminal`, calls `format_alloc(Format::Plain)`,
        splits on `\n`, takes the last `mode.lines.unwrap_or(40)`,
        reads cursor via `terminal.cursor_x()`/`cursor_y()` and
        visibility via `RenderState::cursor_visible()`. Returns
        `LookSnapshotPayload::Lines { snapshot_lines }`.
- [x] 3.3 Define `PtyQuiescenceProbe` in `src/pty/state.rs`
      implementing the cross-transport `WedgeProbe` trait
      (defined in `src/transports/quiescence.rs`, §2.2):
      - `inspect_tail` uses `Formatter::format_alloc(Format::Plain)`
        and slices the last `inspect_lines` rows.
      - `cursor_idle_at(column)` reads
        `terminal.cursor_x()` and compares.
      - `is_settled` returns `true` when no new bytes have
        arrived on the reader thread within `quiet_window`
        (tracked by a shared `Arc<AtomicU64>` last-byte timestamp).
      - `operator_interaction_active` returns `false` (Pty
        semantics; see design Decision).
      The `WedgeProbe::operator_interaction_active` default in
      `src/transports/quiescence.rs` returns `false`; the Pty
      probe can rely on the default (no override needed).

## 4. PtyTransport core

- [x] 4.1 Define `PtyTransport` in `src/pty/transport.rs`:
      - `target_member: BundleMember`
      - `pty_master: Option<Box<dyn Write + Send>>` (raw-write side)
      - `child_pid: Option<u32>`
      - `state: Arc<Mutex<PtyState>>`
      - `output: Arc<PtyOutputView>`
      - `delivery_tx: mpsc::Sender<DeliveryCommand>`
      - `reader_thread: Option<JoinHandle<()>>`
      - `delivery_thread: Option<JoinHandle<()>>`
- [x] 4.2 Implement `Transport::startup(&mut self, context)`:
      - Use `portable_pty::native_pty_system().openpty(...)` with
        per-coder `cols`/`rows`.
      - Build `portable_pty::CommandBuilder` with the per-coder
        `initial-command` or `resume-command` (with
        `{coder-session-id}` substitution), `cwd`, and env vars
        (`COLORTERM=truecolor`, plus a `TERM` value derived from
        the per-coder `term-protocol` field with a default of
        `xterm-256color`; the configurable `term-protocol` surface
        lands in `add-pty-terminal-protocol-config`).
      - `spawn_command(cmd)` returns a `Child`; stash the PID.
      - Clone the master; split into reader / writer.
      - Build `Terminal::new(TerminalOptions { cols, rows,
        max_scrollback: 10_000 })`.
      - Install the effect handlers from §4.3.
      - Spawn the reader thread (reads master → channel →
        `terminal.vt_write`).
      - Spawn the delivery task thread.
      - Publish `WorkerReadinessState::Available` via
        `set_worker_readiness`.
      - Return `TransportStatus { readiness: Ready }`.
- [x] 4.3 Implement effect-handler installation:
      - `on_pty_write` → push bytes onto a `RefCell<Vec<u8>>`
        response buffer; the delivery task drains and writes to
        the master.
      - `on_size` → returns the current `(cols, rows, cell_width,
        cell_height)`.
      - `on_device_attributes` → reports VT220 conformance with
        ANSI color feature.
      - `on_xtversion` → returns `"agentmux-pty <version>"`.
      - `on_title_changed` → publishes a relay stream event
        (injected via the same `StartupContext`-style closure
        pattern ACP uses).
      - `on_bell` → no-op for v1.
- [x] 4.4 Implement `Transport::mailw(&mut self, envelope)`:
      - Enqueue `DeliveryCommand::Mailw(envelope)` onto the
        delivery task's `mpsc::Sender`.
      - Return an `OutcomeFuture` (`oneshot::Receiver<
        SingleDeliveryOutcome>`).
      - The delivery task renders the envelope via
        `DeliveryMessage::render_pane_envelope`, writes to the
        PTY master (then drains `on_pty_write` responses back to
        the master), waits for quiescence via the shared wedge
        state machine from §2, and resolves the future with
        `SingleDeliveryOutcome`.
- [x] 4.5 Implement `Transport::raww(&mut self, content,
      append_enter)`: same envelope-enqueue shape as `mailw` but
      raw bytes; the delivery task flushes any buffered `mailw`
      group first then writes the raw bytes (then a `"\n"` if
      `append_enter`), waits for quiescence, resolves.
- [x] 4.6 Implement `Transport::is_ready(&self) -> bool`:
      child PID is set AND `state.lock().terminal` is initialized.
- [x] 4.7 Implement `Transport::shutdown(&mut self)`:
      - Publish `WorkerReadinessState::Unavailable`.
      - Close the PTY master (causes the reader thread to exit).
      - Send SIGTERM to the child; after a short grace period,
        SIGKILL.
      - `child.wait()` to reap.
      - Drop `state` (drops the terminal and render state).
- [x] 4.8 Implement `Transport::give_output(&self) ->
      Option<Arc<dyn OutputView>>`: returns
      `Some(self.output.clone())`.
- [x] 4.9 Add `TransportImpl::pty(target_member, batch_settings)`
      constructor mirroring `tmux(batch_settings)`.

## 5. TransportImpl wiring (feature-gated)

- [x] 5.1 In `src/transports/contract.rs`, replace the `Pty,` unit
      variant with cfg-gated alternatives:
      ```rust
      #[cfg(feature = "pty")]
      Pty(PtyTransport),
      #[cfg(not(feature = "pty"))]
      Pty,
      ```
      When the `pty` feature is on, the variant carries a real
      `PtyTransport`; when off, the variant stays the existing
      unit-variant stub.
- [x] 5.2 Replace the existing
      `unimplemented!("PTY transport not yet implemented")` arms
      in `startup`, `mailw`, `raww`, `is_ready`, `shutdown`,
      `give_output` with cfg-gated alternatives: when the
      feature is on, the arm delegates to the inner `PtyTransport`
      method (like the existing Tmux arm); when off, the arm
      falls through to today's `unimplemented!(...)`.
- [x] 5.3 Add a `TransportImpl::pty(target_member, batch_settings)`
      constructor (cfg'd on the `pty` feature) mirroring
      `tmux(batch_settings)`.
- [x] 5.4 Re-export `PtyTransport` from `src/transports/mod.rs`
      (cfg'd).

## 6. Configuration

- [x] 6.1 Add `PtyTargetConfiguration` to
      `src/configuration/types.rs`:
      ```rust
      pub struct PtyTargetConfiguration {
          pub initial_command: String,
          pub resume_command: String,
          pub prompt_regex: Option<String>,
          pub prompt_inspect_lines: Option<u16>,
          pub prompt_idle_column: Option<u16>,
          pub cols: u16,        // default 120
          pub rows: u16,        // default 40
          pub prime_timeout_ms: Option<u64>,
          pub wedge_detection: bool,  // default true
      }
      ```
- [x] 6.2 Mirror the same fields through `RawPtyTarget` in
      `src/configuration/raw.rs` (deserialize from
      `[coders.<id>.pty]`).
- [x] 6.3 Validator (`src/configuration/targets.rs`):
      - Reject `prime-timeout-ms = 0` (same rule as Tmux).
      - Reject `cols = 0` or `rows = 0`.
      - Per-coder validator enforces exactly one of
        `[coders.<id>.tmux]` / `[coders.<id>.pty]` (not both, not
        neither). Reuse the existing per-coder mutual-exclusion
        check that today covers `[coders.<id>.tmux]` /
        `[coders.<id>.acp]`.
- [x] 6.4 In `src/relay/handlers/sender.rs`, construct a
      `PtyTargetConfiguration` (analogous to the existing
      `TmuxTargetConfiguration` construction) when the bundle
      member is Pty-backed.

## 7. Session type taxonomy

- [x] 7.1 Add `SessionType::Pty` variant. The capability row
      `look=true, write=true, stream=true, choices=false` is
      already declared in `session-relay/spec.md` (it is the
      forward-looking row); the spec delta updates that note to
      say "Pty is a populated transport" instead of "no Pty
      session type exists yet."

## 8. Worker readiness

- [ ] 8.1 Pty's worker thread publishes `WorkerReadinessState`
      transitions via the existing `set_worker_readiness` /
      `publish_worker_readiness` functions in
      `src/relay/`. Mirror ACP's wiring:
      - `Initializing` (briefly, between worker spawn and first
        effect-handler registration).
      - `Available` on successful `startup`.
      - `Busy` while a flush group is in flight (delivery task
        owns the writer).
      - `Unavailable` on `SendOutcome::Failed` with
        `reason_code = "pane_wedged"`, or on child exit.
      - `Recovering` on respawn-after-child-exit.

## 9. Unit tests

- [x] 9.1 Register `tests/unit/pty_transport.rs` in
      `tests/unit.rs`.
- [x] 9.2 Implement the five behavior-class probes (always
      unresponsive, always wedge, pending choice, slow prompt,
      normal flow) for Pty's generalized wedge/prime state
      machine. Mirror the Tmux probe test surface so the same
      five scenarios exercise Pty's path.
- [x] 9.3 Add coalesce-during-wedge-counter scenarios (counter
      increments, resets on signature change, fires at 3).
- [ ] 9.4 Add coalesce-during-prime-does-not-extend-window
      scenario.
- [x] 9.5 Add wedge-outcome-maps-to-pane_wedged and
      prime-timeout-outcome-maps-to-Timeout outcome-mapping
      scenarios.
- [x] 9.6 Add wedge-default-on and wedge-disabled scenarios.
- [x] 9.7 Add prime-timeout-default-off and prime-timeout-opt-in
      scenarios.
- [ ] 9.8 Add the wedge-disabled + prime-timeout-set scenario
      (prime-timeout bounds every quiescent state when wedge is
      disabled) — matches the existing Tmux spec scenario.
- [x] 9.9 Add a `PtyTransport`-level integration test that
      spawns `cat` under portable-pty, writes a line, captures
      the snapshot via `PtyOutputView::look`, and asserts the
      line appears in `snapshot_lines`. This is the closest
      unit-test analog to the real-world round-trip validated
      by the `agentmux-pty` POC.

## 10. Operator tooling

- [x] 10.1 Update `README.md`:
      - "Requirements" section gains a note about Zig 0.15.x for
        Pty users; document `GHOSTTY_SOURCE_DIR` and
        `GHOSTTY_ZIG_SYSTEM_DIR` overrides for sandboxed CI.
      - CLI Surface section documents the parallel
        `[coders.<id>.pty]` coder config table.
      - MCP Surface section notes that Pty sessions use the same
        look bounds as Tmux.
- [x] 10.2 Update `AGENTS.md` Prerequisites section to mention
      Zig 0.15.x.
- [x] 10.3 Promote `src/bin/agentmux_pty.rs` from "throwaway
      spike" to "operator smoke-test entry point." Update the
      file's module-level doc comment to reflect this.

## 11. Validation

- [x] 11.1 `cargo test --lib` and `cargo test --tests` pass
      with no regressions in the existing 17 lib tests and
      320+ integration tests.
- [x] 11.2 `cargo clippy --all-targets --no-deps` is silent.
- [x] 11.3 `cargo fmt --check` is silent.
- [x] 11.4 `openspec validate add-pty-transport --strict` is
      valid.
- [x] 11.5 `cargo run --bin agentmux-pty -- /bin/bash`
      round-trips a real shell prompt through libghostty-vt.
- [ ] 11.6 Pty-configured coder session in a real bundle
      delivers a `mailw` envelope to a child shell and resolves
      `SendOutcome::Delivered` within the configured
      `quiet_window` (manual smoke test against a real bundle).
- [ ] 11.7 Pty `look` returns a `LookSnapshotPayload::Lines {
      snapshot_lines }` consistent with the captured screen
      (manual smoke test against `cat /etc/hostname`).
- [ ] 11.8 Pty wedge detection fires on a wedged pane with
      default-on config (manual smoke test using `cat > /dev/full`
      or equivalent).
- [ ] 11.9 Pty prime timeout fires on an unresponsive pane when
      configured (manual smoke test using `sleep infinity` as
      the child command).
- [ ] 11.10 Pty wedge-disabled + prime-timeout-set scenario
      matches the Tmux spec's `Scenario: Tmux prime timeout
      bounds post-quiescence wait when wedge is disabled` (manual
      smoke test).

## 12. CI feature-gating (immediate follow-up, not in this proposal's implementation)

This section is a follow-up that lands as `todos/relay/98`
immediately after this proposal merges. It is NOT in the
proposal's implementation tasks because the CI adjustment
involves the GitHub Actions workflow YAML and the lint/test
runner configuration, which is infrastructure that the BE or
Coordinator typically owns rather than the Pty Specialist.

- [x] 12.1 Install Zig 0.15.x in the lint runner (the existing
      `.github/workflows/tester.yaml` `lint` job). The runner
      needs the `zig` binary on `PATH` and outbound network
      access to `github.com/ghostty-org/ghostty.git` (or the
      `GHOSTTY_SOURCE_DIR` / `GHOSTTY_ZIG_SYSTEM_DIR` overrides
      set via workflow env if hermetic CI is preferred).
- [x] 12.2 Add a `pty` matrix entry to the `test` job's
      `matrix.platform` config so Pty gets CI coverage. Without
      `--all-features` on the test invocation, the `pty` feature
      is otherwise invisible to CI.
- [x] 12.3 Document the CI behavior in `.github/workflows/tester.yaml`
      with a comment explaining why `pty` is a separate matrix
      entry (Zig dep + ghostty clone at build time).
- [x] 12.4 Verify the CI configuration: open a PR, observe both
      the default-feature and `pty`-feature matrix entries run
      successfully, observe lint passes with the `pty` feature
      enabled.

Acceptance: a PR with the `pty` feature enabled in `Cargo.toml`
passes lint + test in CI; a PR with the default feature set (no
`pty`) also passes lint + test (this is the existing baseline and
must continue to pass).