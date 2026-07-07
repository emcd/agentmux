## 1. Bootstrap / spawned-child capture

- [ ] 1.1 Surface the spawned relay child (pid) from `spawn_relay_host_for_tui`
      to `ensure_tui_relay_available`, gated on `BootstrapReport.spawned_relay`.
- [ ] 1.2 Start the auto-spawned relay in its own process group
      (`Command::process_group(0)`) so terminal signals do not implicitly reach it.

## 2. Teardown wiring

- [ ] 2.1 Thread the owned relay pid (present only when auto-spawned) out of
      `ensure_tui_relay_available` to `run_agentmux_tui`.
- [ ] 2.2 After `crate::tui::run` returns (any exit path), if the TUI owns the
      relay, send SIGTERM to the owned pid and wait bounded for teardown; skip
      entirely when the relay was already running at startup.

## 3. Specs & docs

- [ ] 3.1 Fold the `runtime-bootstrap` and `tui-surface` deltas into `specs/` at
      archive time.
- [ ] 3.2 Document the residual teardown risk (spawned relay is an ad hoc
      single-operator convenience; use systemd / `agentmux host relay` for a
      durable relay) in `documentation/usage/tui.md`.

## 4. Tests

- [ ] 4.1 Test: a TUI-auto-spawned (owned) relay receives the graceful stop on
      TUI exit.
- [ ] 4.2 Test: an already-running relay is not signaled on TUI exit — the
      spawned-vs-already-running branch distinction.
- [ ] 4.3 Confirm the reused stop path prunes owned sessions and reaps the tmux
      server (existing `relay_sigint_prunes_owned_sessions_and_reaps_tmux_server`
      coverage; extend only if the TUI-driven path is not already exercised).
