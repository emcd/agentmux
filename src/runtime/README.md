# Runtime Bootstrap

This directory contains runtime bootstrap and environment-resolution modules
shared by relay and MCP hosts.

## Modules

- `paths.rs`
  - resolves config/state/inscriptions roots,
  - resolves per-bundle sockets and lock paths,
  - enforces ownership and secure directory permissions.
- `association.rs`
  - resolves bundle/session association for MCP + CLI workflows,
  - supports precedence: CLI flags > local overrides > auto-discovery.
- `tui_sender.rs`
  - resolves TUI sender precedence from CLI + sender config files + association
    fallback.
- `bootstrap.rs`
  - relay socket bind and runtime lock acquisition.
- `inscriptions.rs`
  - process/bundle inscription path setup and event emission helpers.
- `starter.rs`
  - hydrates starter config files when missing:
    - `<config-root>/coders.toml`
    - `<config-root>/policies.toml`
    - `<config-root>/bundles/example.toml`
- `signals.rs`
  - process signal wiring and shutdown state checks.
- `error.rs`
  - shared runtime error taxonomy and helpers.
- `mod.rs`
  - module exports.

## Association Override File

Per-worktree overrides are loaded from:

- `.auxiliary/configuration/agentmux/overrides/mcp.toml`

Supported keys:

- `bundle_name`
- `session_name`
- `config_root`

## TUI Sender Override File

Per-worktree TUI sender overrides are loaded from:

- `.auxiliary/configuration/agentmux/overrides/tui.toml`

Supported key:

- `sender`

## Bootstrap Notes

- `bootstrap.rs` uses spawn-lock and runtime-lock files to avoid duplicate relay
  startup and to clean stale sockets safely.
- Relay startup can be disabled at call sites (`BootstrapOptions`) for
  process-only or diagnostics scenarios.
