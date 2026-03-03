# Change: Add MCP association auto-discovery and local overrides

## Why

Current MCP startup still relies on explicit flag usage and permissive sender
fallback behavior. For multi-worktree development, this is repetitive and
fragile.

`tmuxmux` should infer bundle and sender identity automatically from local
workspace context, while still supporting explicit testing and cross-project
coordination overrides.

## What Changes

- Add startup-time bundle auto-discovery for MCP servers.
- Add startup-time sender-session auto-discovery for MCP servers.
- Standardize explicit MCP association flags:
  - `--bundle-name`
  - `--session-name`
- Define precedence between CLI flags, local override file, and auto-discovery.
- Add optional local override file:
  - `.auxiliary/configuration/tmuxmux/overrides/mcp.toml`
- Ensure `.auxiliary/configuration/tmuxmux/overrides/` is Git-ignored so
  per-worktree overrides do not leak into shared commits.
- Require startup failure with structured bootstrap errors when bundle/sender
  association is missing or ambiguous.

## Impact

- Affected specs:
  - `runtime-bootstrap` (modified)
- Affected code:
  - `src/bin/tmuxmux-mcp.rs`
  - `src/configuration.rs`
  - runtime/bootstrap error mapping and diagnostics
