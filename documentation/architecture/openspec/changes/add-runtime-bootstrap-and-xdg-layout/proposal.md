# Change: Add runtime bootstrap and XDG state/config layout

## Why

`tmuxmux` needs predictable local runtime behavior across MCP servers and
future human clients. Without a standardized bootstrap flow and path layout,
multi-process startup can race, sockets can become stale, and deployments can
drift across machines.

## What Changes

- Add a new `runtime-bootstrap` capability for relay startup and discovery.
- Define XDG-compliant configuration and state roots for `tmuxmux`.
- Define debug-build repository-local state override support for isolated
  development runtime testing.
- Define per-bundle runtime directories with:
  - `tmux.sock` for tmux server control.
  - `relay.sock` for client-to-relay IPC.
- Define MCP-side relay auto-start behavior with lock-based spawn
  coordination, stale-socket handling, and startup timeout.
- Define sender-session association bootstrap from MCP runtime context with
  working-directory matching as best-effort inference.
- Define local security posture for runtime artifacts (same-user ownership and
  restrictive permissions).

## Impact

- Affected specs: `runtime-bootstrap` (new capability).
- Related specs:
  - `session-relay` from `add-mcp-session-relay-mvp`
  - `mcp-tool-surface` from `add-mcp-tool-surface-contract`
- Affected code:
  - path resolution and configuration loading
  - relay process manager and spawn coordination
  - MCP bootstrap path for relay connectivity and sender association
