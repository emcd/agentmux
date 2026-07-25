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
- `tui_session.rs`
  - resolves TUI session selection from CLI + `users.toml` defaults, and
    browsing-bundle selection from CLI + `ui.toml` defaults,
  - validates selected TUI session policy references.
- `bootstrap.rs`
  - relay socket bind and runtime lock acquisition.
- `inscriptions.rs`
  - process/bundle inscription path setup and event emission helpers.
    `append_inscription_record` is the single public write seam: it builds the
    record (timestamp + pid + event + details) and trailing newline as one
    `String` and commits the full record with a single `write_all` against a
    `File` opened with `O_APPEND` (`OpenOptions::create(true).append(true)`).
    Concurrent emitters append independently — the kernel serializes the writes
    on the open file description — so each record lands as one atomic line,
    which is required for downstream JSON-per-line readers to drop nothing and
    parse every line.
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

## Root Resolution

The configuration root resolves by precedence, each tier ranked by how
deployment-specific its source is:

1. `--configuration-directory`
2. `AGENTMUX_CONFIGURATION_DIRECTORY`
3. nearest-ancestor discovery, only with `--discover-local-configuration`
4. `$XDG_CONFIG_HOME/agentmux`, else `~/.config/agentmux`

Tiers 1 and 2 **replace** the root; they do not extend a search list, so a root
supplied explicitly never falls through to another one for a file it does not
define. Resolution does not vary by build profile.

Discovery walks the canonicalized working directory and its ancestors for
`<ancestor>/.auxiliary/configuration/agentmux`, nearest winning, and reports its
selection on stderr — never stdout, which `host mcp` uses for the protocol.

Starter hydration applies only to a root from tier 4. A root named by flag,
environment, or discovery is never scaffolded; when it does not exist, that is a
fault rather than a reason to create one.

The **state** and **inscriptions** roots deliberately keep their build-profile
gating and their Git-derived repository-root provenance. That gating is
currently the only thing keeping a source-tree relay and an installed relay off
the same relay-wide socket, locks, ready sentinel, principal store, and peer
credentials. Runtime instances replace it; until then the configuration root and
the state root resolve by different rules on purpose.

## Association Override File

Per-worktree overrides are loaded from:

- `.auxiliary/configuration/agentmux/overrides/mcp.toml`

Supported keys:

- `bundle_name`
- `session_name`
- `config_root`

## TUI Session Override File

Per-worktree TUI session overrides are loaded from:

- `.auxiliary/configuration/agentmux/overrides/users.toml`

Supported keys:

- `default-session`
- `[[sessions]]`

The override is identity-only. UI-surface defaults (`default-bundle`) live in
`ui.toml`, which is read from the configuration root only and has no per-worktree
override.

## Bootstrap Notes

- `bootstrap.rs` uses spawn-lock and runtime-lock files to avoid duplicate relay
  startup and to clean stale sockets safely.
- Relay startup can be disabled at call sites (`BootstrapOptions`) for
  process-only or diagnostics scenarios.
