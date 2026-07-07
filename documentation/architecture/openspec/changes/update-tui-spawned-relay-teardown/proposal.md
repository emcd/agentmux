# Change: TUI stops the relay it auto-spawned on exit

## Why

`agentmux tui` auto-spawns a relay when none is reachable, but never stops it:
`ensure_tui_relay_available` discards both `BootstrapReport.spawned_relay` and
the spawned child handle. Closing a standalone TUI therefore leaves an orphaned
relay — and the tmux/pty coder sessions it owns — running. With the relay now
normally supervised by systemd, auto-spawn is an ad hoc single-operator
fallback; for that narrow case "you brought it up, you can bring it down" is the
least-surprise contract.

## What Changes

- Track `BootstrapReport.spawned_relay` through `ensure_tui_relay_available` and
  capture the auto-spawned relay's process id.
- On TUI exit (any exit path), if — and only if — the TUI auto-spawned the relay,
  send it a graceful termination signal, reusing the relay's existing
  SIGINT/SIGTERM shutdown path that prunes owned tmux sessions and reaps the
  tmux server. If the relay was already running (systemd or otherwise), leave it
  untouched — today's behavior.
- Spawn the auto-started relay in its own process group so its lifecycle is
  driven solely by the TUI's explicit signal, not by incidental terminal Ctrl-C
  propagation.
- **BREAKING** (behavioral): a TUI-auto-spawned relay no longer outlives the TUI.
- Document the residual risk in usage docs: a TUI-spawned relay is an ad hoc
  single-operator convenience whose shutdown actively tears down the coder
  sessions it owns; a durable/shared relay must be run via systemd or
  `agentmux host relay` directly.

## Impact

- Affected specs: `runtime-bootstrap` (TUI Auto-Spawn Relay Lifecycle
  Ownership), `tui-surface` (TUI Transport Failure Semantics)
- Affected code: `src/commands/tui.rs` (`run_agentmux_tui`,
  `ensure_tui_relay_available`, `spawn_relay_host_for_tui`),
  `src/runtime/bootstrap.rs` (spawned-child capture surface), the relay
  signal-driven shutdown path (`src/relay/lifecycle.rs::shutdown_bundle_runtime`,
  reused unchanged), `documentation/usage/tui.md`
- Related (out of scope): `ideas/transports/5` — decoupling transport-host
  processes from relay lifetime, which makes hard-stop non-destructive and
  retires this risk entirely.
