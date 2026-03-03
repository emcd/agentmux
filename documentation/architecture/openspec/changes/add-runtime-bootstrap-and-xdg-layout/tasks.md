## 1. Implementation

- [x] 1.1 Implement XDG config root resolution with fallback to
      `~/.config/tmuxmux`.
- [x] 1.2 Implement XDG state root resolution with fallback to
      `~/.local/state/tmuxmux`.
- [x] 1.3 Implement debug-build repository-local state override support for
      `<repo_root>/.auxiliary/state/tmuxmux`.
- [x] 1.4 Implement per-bundle runtime path builder at
      `<state_root>/bundles/<bundle_name>/`.
- [x] 1.5 Use per-bundle `tmux.sock` and `relay.sock` paths in runtime
      operations.
- [x] 1.6 Implement relay bootstrap lock files (`relay.lock`,
      `relay.spawn.lock`) and single-spawner coordination.
- [x] 1.7 Implement MCP-side relay connectivity gate with connect-first and
      fail-fast behavior (no MCP auto-spawn).
- [x] 1.8 Add reusable bootstrap configuration for `auto_start_relay` and
      `startup_timeout_ms` with documented defaults for non-MCP clients.
- [x] 1.9 Implement sender association resolver with explicit-session override
      and working-directory match fallback.
- [x] 1.10 Return structured bootstrap errors for unknown/ambiguous sender
      association and relay startup timeout/failure.
- [x] 1.11 Enforce runtime artifact ownership and restrictive permission
      posture for bundle runtime directories and sockets.
- [ ] 1.12 Add integration tests for concurrent client bootstrap,
      stale-socket cleanup, and sender association resolution.
- [ ] 1.13 Add integration tests for debug-build repository-local state
      override selection.
- [x] 1.14 Add documentation for XDG layout, bundle runtime paths, startup
      behavior, and debug-build repository-local override usage.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
