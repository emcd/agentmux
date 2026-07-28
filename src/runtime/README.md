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
  - bundle precedence: `--bundle` > injected environment > effective association
    file > `--default-bundle`,
  - session precedence: `--session-name` > injected environment > effective
    association file > working-directory match against declared member
    directories,
  - resolves to nothing rather than guessing when no tier supplies an identity.
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

The configuration **layer list** resolves by precedence, each tier ranked by how
deployment-specific its source is:

1. `--configuration-directory`, repeatable; each occurrence appends one layer
2. `AGENTMUX_CONFIGURATION_DIRECTORY`, a `:`-separated list in the same order
3. `$XDG_CONFIG_HOME/agentmux`, else `~/.config/agentmux` — a single layer

Tiers 1 and 2 **replace** the list; they do not extend it, so a supplied list is
closed and never falls through to a root the operator did not name. Within a
list the **first** layer wins, matching every other Unix search path. Tier 3
resolves as a single-layer list, so one lookup path serves every tier.
Resolution does not vary by build profile.

A path containing `:` cannot be expressed through the environment form; the
repeatable flag is the escape hatch. An empty element is rejected in either
form rather than read as the working directory.

Starter hydration applies only to a list from tier 3, which is a single layer. A
list supplied by flag or environment is never scaffolded; when one of its layers
does not exist, that is a fault rather than a reason to create one.

The **state** and **inscriptions** roots deliberately keep their build-profile
gating and their Git-derived repository-root provenance. That gating is
currently the only thing keeping a source-tree relay and an installed relay off
the same relay-wide socket, locks, ready sentinel, principal store, and peer
credentials. Runtime instances replace it; until then the configuration root and
the state root resolve by different rules on purpose.

That provenance has exactly one resolver, `repository_checkout_root`. Every
surface — CLI, TUI, `host mcp`, `host relay` — goes through it, because a
surface answering differently would look for the relay socket somewhere the
relay never bound it. It asks Git for the common directory and takes the
repository root owning it, then requires that root's `Cargo.toml` to declare
`name = "agentmux"`. Git makes the answer identical from every worktree of a
checkout, so siblings share one relay instead of each starting its own, and it
searches ancestors so any working directory beneath a checkout resolves it. The
manifest marker confines the repository-local branch to an actual Agentmux
checkout rather than whichever repository the process happens to stand in; when
Git resolves a repository that fails the check, the reason is reported on
stderr. Stderr is the only channel available, because root resolution runs
before any inscriptions sink is configured. Worktrees owned by a bare
repository are not supported and resolve production paths: their common
directory is conventionally `<name>.git` rather than an ancestor named `.git`,
and a bare repository has no checked-out root to carry the manifest. An explicit
`--repository-root` is operator intent and bypasses the resolver. In release
builds it always answers `None`.

## Overridable Files

Every configuration file resolves through the same layer list, so overriding one
is the same operation regardless of which file it is. For layers `[A, B]`:

| Logical artifact | Effective lookup |
|---|---|
| `mcp.toml` (association) | `A/mcp.toml`, then `B/mcp.toml` |
| `users.toml` (TUI identity) | `A/users.toml`, then `B/users.toml` |
| `ui.toml` (surface defaults) | `A/ui.toml`, then `B/ui.toml` |
| `bundles/<id>.toml` | `A`'s entry shadows `B`'s entry of the same id |

`mcp.toml` supports `bundle_name` and `session_name`. It no longer carries a
configuration-root field: the file lives *under* a configuration layer, so
letting it select the layers made the lookup circular. Roots resolve first, and
the file is read from the resolved list.

Layering is whole-file replacement, not deep merge — "what is in effect" stays
answerable by naming one file. Bundle *directories* are the exception and union
by identifier, since replacing the directory wholesale would force a layer
redefining one bundle to restate every other one.

## Bootstrap Notes

- `bootstrap.rs` uses spawn-lock and runtime-lock files to avoid duplicate relay
  startup and to clean stale sockets safely.
- Relay startup can be disabled at call sites (`BootstrapOptions`) for
  process-only or diagnostics scenarios.
