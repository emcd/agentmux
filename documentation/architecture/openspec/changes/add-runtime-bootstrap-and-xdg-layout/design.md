## Context

The relay and MCP tool-surface proposals define behavior and public contracts.
This change defines local process bootstrap and filesystem layout so multiple
MCP servers can safely converge on one bundle runtime.

The design uses a lock-coordinated bootstrap pattern:

- non-blocking spawn lock for single-spawner coordination
- re-check after lock acquisition
- stale-socket cleanup before spawn
- wait-until-connectable timeout gate

## Goals / Non-Goals

- Goals:
  - Standardize where `tmuxmux` reads config and writes runtime state.
  - Provide deterministic MCP relay connectivity checks at startup.
  - Preserve race-safe relay auto-start primitives for future non-MCP clients.
  - Keep one tmux socket and one relay socket per bundle.
  - Preserve same-host, same-user trust assumptions.
- Non-Goals:
  - Cross-host relay discovery.
  - Multi-user authentication and authorization.
  - Durable queue persistence in this change.

## Decisions

- Decision: use XDG-compliant roots.
  - Config root:
    - `$XDG_CONFIG_HOME/tmuxmux`, fallback `~/.config/tmuxmux`
  - State root:
    - `$XDG_STATE_HOME/tmuxmux`, fallback `~/.local/state/tmuxmux`
  - Rationale: predictable platform-standard paths.

- Decision: debug builds support repository-local state override.
  - Override path:
    - `<repo_root>/.auxiliary/state/tmuxmux`
  - Scope:
    - debug/development mode only
  - Rationale: enables testing experimental or breaking changes without
    disrupting deployed XDG-based runtime state.

- Decision: use one runtime directory per bundle.
  - Directory:
    - `<state_root>/bundles/<bundle_name>/`
  - Runtime artifacts:
    - `tmux.sock`
    - `relay.sock`
    - `relay.lock`
    - `relay.spawn.lock`
  - Rationale: isolates bundle runtimes and avoids cross-bundle collisions.

- Decision: MCP bootstrap is connect-only and fail-fast.
  - Behavior:
    - MCP tries `relay.sock`.
    - If unavailable, startup fails with a structured remediation error.
  - Rationale: avoids MCP-driven bootstrap loops and keeps startup ownership
    explicit.

- Decision: keep relay auto-start primitives for non-MCP clients.
  - Behavior:
    - bootstrap helper supports optional spawn, lock coordination, stale-socket
      cleanup, and startup timeout.
    - intended for future TUI/CLI flows rather than MCP startup path.
  - Config:
    - `bootstrap.auto_start_relay` (default: `true`)
    - `bootstrap.startup_timeout_ms` (default: `10000`)
  - Rationale: preserves tested startup mechanics for later interactive
    clients without coupling MCP to relay lifecycle.

- Decision: spawn coordination uses lock files.
  - Behavior:
    - exactly one process acquires `relay.spawn.lock` and spawns relay
    - other contenders wait for `relay.sock` connectability
  - Rationale: avoids duplicate relay processes and startup races.

- Decision: sender association resolves from MCP context.
  - Resolution order:
    - explicit configured sender session (if provided)
    - best-effort working-directory match against bundle member
      `working_directory`
  - Failure behavior:
    - no match or ambiguous matches produce a structured bootstrap error
  - Rationale: keeps `chat` requests clean while preserving deterministic
    identity mapping.

- Decision: runtime files are same-user restricted.
  - Required posture:
    - bundle runtime directory mode `0700`
    - socket files mode `0600` where supported
    - existing runtime artifacts must be owned by current effective user
  - Rationale: local security boundary and least-privilege defaults.

## Risks / Trade-offs

- Connect-only MCP requires explicit operator startup order.
- State-root mismatches between relay and MCP can fail startup until corrected.
- Working-directory inference can be ambiguous in shared repos/worktrees.
- Enforcing restrictive permissions may expose misconfigured environments.

## Migration Plan

1. Implement XDG path resolver and bundle runtime directory helper.
2. Implement relay bootstrap coordinator with lock-based single spawner.
3. Implement sender association resolver for MCP startup context.
4. Update MCP bootstrap to use connect-only relay availability checks.
5. Add operational guidance for startup ordering and shared state-root usage.
