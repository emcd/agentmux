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
- `bootstrap.rs`
  - relay socket bind and runtime lock acquisition.
- `inscriptions.rs`
  - process/bundle inscription path setup and event emission helpers.
- `starter.rs`
  - hydrates starter config files (`coders.toml`, `bundles/example.toml`) when
    missing.
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
