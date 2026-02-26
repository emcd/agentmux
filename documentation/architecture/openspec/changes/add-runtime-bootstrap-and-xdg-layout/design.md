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
  - Provide deterministic, race-safe relay auto-start from MCP servers.
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

- Decision: default MCP bootstrap is auto-start enabled.
  - Behavior:
    - MCP first tries `relay.sock`.
    - If unavailable, MCP attempts relay startup unless auto-start is disabled.
  - Config:
    - `bootstrap.auto_start_relay` (default: `true`)
    - `bootstrap.startup_timeout_ms` (default: `10000`)
  - Rationale: matching user expectation that clients "just work."

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

- Auto-start improves UX but can hide failures until timeout boundaries.
- Working-directory inference can be ambiguous in shared repos/worktrees.
- Enforcing restrictive permissions may expose misconfigured environments.

## Migration Plan

1. Implement XDG path resolver and bundle runtime directory helper.
2. Implement relay bootstrap coordinator with lock-based single spawner.
3. Implement sender association resolver for MCP startup context.
4. Update MCP bootstrap to use relay auto-start policy and timeout handling.
5. Add docs for path layout and operational troubleshooting.
