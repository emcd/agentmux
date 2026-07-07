## 1. Bootstrap / spawned-child capture

- [x] 1.1 Surface the spawned relay child from `spawn_relay_host_for_tui` to
      `ensure_tui_relay_available`, gated on `BootstrapReport.spawned_relay`
      (new `runtime::bootstrap::SpawnedRelay` owns the child).
- [x] 1.2 Start the auto-spawned relay in its own process group
      (`Command::process_group(0)`, `#[cfg(unix)]`) so terminal signals do not
      implicitly reach it.

## 2. Teardown wiring

- [x] 2.1 Thread the owned relay (present only when auto-spawned) out of
      `ensure_tui_relay_available` to `run_agentmux_tui` as `Option<SpawnedRelay>`.
- [x] 2.2 After `crate::tui::run` returns (any exit path), if the TUI owns the
      relay, `SpawnedRelay::stop` sends SIGTERM and waits bounded for teardown;
      skipped entirely when the relay was already running at startup.

## 3. Specs & docs

- [ ] 3.1 Fold the `runtime-bootstrap` and `tui-surface` deltas into `specs/` at
      archive time.
- [x] 3.2 Document the residual teardown risk (spawned relay is an ad hoc
      single-operator convenience; use systemd / `agentmux host relay` for a
      durable relay) in `documentation/usage/tui.md`.

## 4. Tests

- [x] 4.1 Test: a TUI-auto-spawned (owned) relay is stopped via the graceful
      SIGTERM path (`SpawnedRelay::stop` terminates + reaps within grace).
- [x] 4.2 Test: an already-running relay yields no ownership and is not signaled
      — the spawned-vs-already-running branch distinction, exercised through the
      public `bootstrap_relay` surface.
- [x] 4.3 The reused stop path's session pruning / tmux-server reaping is already
      covered by `relay_sigint_prunes_owned_sessions_and_reaps_tmux_server`;
      `SpawnedRelay::stop` reuses that exact SIGTERM path, so no relay-side test
      is duplicated here.
