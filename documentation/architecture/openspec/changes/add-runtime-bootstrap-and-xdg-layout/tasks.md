## 1. Implementation

- [ ] 1.1 Implement XDG config root resolution with fallback to
      `~/.config/tmuxmux`.
- [ ] 1.2 Implement XDG state root resolution with fallback to
      `~/.local/state/tmuxmux`.
- [ ] 1.3 Implement debug-build repository-local state override support for
      `<repo_root>/.auxiliary/state/tmuxmux`.
- [ ] 1.4 Implement per-bundle runtime path builder at
      `<state_root>/bundles/<bundle_name>/`.
- [ ] 1.5 Use per-bundle `tmux.sock` and `relay.sock` paths in runtime
      operations.
- [ ] 1.6 Implement relay bootstrap lock files (`relay.lock`,
      `relay.spawn.lock`) and single-spawner coordination.
- [ ] 1.7 Implement MCP-side relay auto-start flow with connect-first,
      lock-coordinated spawn, stale-socket cleanup, and timeout wait.
- [ ] 1.8 Add bootstrap configuration for `auto_start_relay` and
      `startup_timeout_ms` with documented defaults.
- [ ] 1.9 Implement sender association resolver with explicit-session override
      and working-directory match fallback.
- [ ] 1.10 Return structured bootstrap errors for unknown/ambiguous sender
      association and relay startup timeout/failure.
- [ ] 1.11 Enforce runtime artifact ownership and restrictive permission
      posture for bundle runtime directories and sockets.
- [ ] 1.12 Add integration tests for concurrent MCP bootstrap, stale-socket
      cleanup, and sender association resolution.
- [ ] 1.13 Add integration tests for debug-build repository-local state
      override selection.
- [ ] 1.14 Add documentation for XDG layout, bundle runtime paths, startup
      behavior, and debug-build repository-local override usage.

## 2. Validation

- [ ] 2.1 Run `hatch --env develop run linters`.
- [ ] 2.2 Run `hatch --env develop run testers`.
