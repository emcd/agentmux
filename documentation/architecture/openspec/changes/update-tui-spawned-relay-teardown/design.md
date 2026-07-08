## Context

`agentmux tui` calls `ensure_tui_relay_available`, which invokes
`bootstrap_relay`. When no relay socket is reachable and this process wins the
spawn lock, the spawn closure (`spawn_relay_host_for_tui`) starts
`agentmux host relay` as a child and `bootstrap_relay` returns
`BootstrapReport { spawned_relay: true }`. Today the report and the child handle
are both discarded, so nothing stops the relay when the TUI exits.

The relay's graceful shutdown is not a passive socket close: its
SIGINT/SIGTERM handler path runs `shutdown_bundle_runtime`
(`src/relay/lifecycle.rs`), which `prune_owned_session`s every tmux session the
relay owns and reaps the tmux server when it becomes unowned. In-process pty
children die with the relay. So "stop what you spawned" is real, active teardown
of coder sessions — bounded to the narrow auto-spawn case.

## Goals / Non-Goals

- Goals:
  - Deterministic teardown of a TUI-auto-spawned relay on TUI exit, across both
    quit-key and signal exit paths.
  - Preserve today's hands-off behavior for a relay that was already running.
- Non-Goals:
  - Changing systemd or `agentmux host relay` lifecycle.
  - Eliminating the destructive-teardown risk itself — decoupling transport-host
    processes from relay lifetime (`ideas/transports/5`) is the real fix and is
    out of scope here.
  - Reference-counting multiple concurrent TUIs against one shared relay;
    auto-spawn is single-operator by construction and the spawn lock already
    serializes to exactly one spawner.

## Decisions

- Decision: Ownership is `BootstrapReport.spawned_relay == true` — i.e. this
  process's spawn closure ran and produced the child. Contenders that merely
  waited for readiness get `spawned_relay = false` and never signal, so at most
  the single spawner owns teardown.
- Decision: Capture the spawned relay's pid from the `Child` we start, not from
  the runtime lock file. The lock file's diagnostic pid is best-effort and races
  with relay startup; the child we spawned is authoritative.
- Decision: Spawn the auto-started relay in its own process group
  (`Command::process_group(0)`). Today the child shares the TUI's process group,
  so a terminal SIGINT already reaches it — the current "keep running after
  exit" contract does not actually hold for Ctrl-C exits. Isolating the group
  makes the TUI the sole author of the spawned relay's shutdown and covers the
  quit-key exit path uniformly.
- Decision: Reuse the relay's existing signal-driven shutdown
  (`install_shutdown_signal_handlers` -> `shutdown_bundle_runtime`) by sending
  SIGTERM to the owned pid. No new relay control surface. Wait bounded for
  teardown; do not block TUI exit past the relay's own watchdog grace window.
- Alternatives considered:
  - Rely on process-group SIGINT propagation with no explicit signal: rejected —
    non-deterministic, misses the quit-key exit path, and couples teardown to tty
    semantics.
  - Add a relay socket "shutdown" control message: rejected — new API surface for
    what a signal already does cleanly.
  - Read the relay pid from the runtime lock file: rejected — racy, best-effort
    diagnostic value.

## Risks / Trade-offs

- Teardown actively prunes owned tmux sessions and kills in-process pty children,
  so an operator who auto-spawned a relay via the TUI loses those sessions on
  exit. -> Mitigation: document explicitly; scope strictly to the auto-spawn
  branch; the durable systemd path is unaffected.
- Signal delivery / teardown could hang. -> Mitigation: bounded wait leaning on
  the relay's own watchdog grace; the TUI never blocks indefinitely.
- `ideas/transports/5` will later make hard-stop non-destructive, shifting the
  risk calculus. -> Noted as the end state; this is the interim alpha call.

## Migration Plan

Alpha software: no back-compat shim. Behavior flips for the auto-spawn branch
only; the already-running branch is unchanged.

## Open Questions

- SIGTERM vs SIGINT for the explicit stop: both are handled identically by
  `install_shutdown_signal_handlers`. Default to SIGTERM (conventional
  programmatic stop); confirm during implementation there is no TUI-specific
  reason to prefer SIGINT.
