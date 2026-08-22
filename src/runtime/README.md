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
  - process signal wiring and shutdown state checks, plus the process-wide
    **shutdown-work deadline**. Shutdown is one budget, not a set of unrelated
    durations: every bounded step takes its share via `budget_within_shutdown`,
    which fits a configured bound to what remains and withholds a reserve for the
    steps behind it.

    The deadline is established at the *first* of two events — the watchdog
    observing the shutdown flag, or the first step needing a budget once
    shutdown has been requested — which is why `register_shutdown_grace` runs
    when the watchdog is spawned rather than when it fires. That is not the same
    instant as the watchdog's forced exit and must not be documented as if it
    were: it is earlier by however long the watchdog has yet to observe, and
    earlier is the required direction. First-arming-wins keeps the earliest.

    Three states, not two, and conflating any pair is a defect rather than a
    simplification. A registered grace is the positive fact that this process
    *will* have a deadline, so `None` from `shutdown_time_remaining` means
    "never" (CLI and test harnesses — take the configured bound) rather than
    "not yet"; `Some(ZERO)` means the grace is spent, and collapsing it into
    `None` would restore a full wait exactly when there is least time for it. See
    `src/relay/README.md` for the nesting diagram and why it is load-bearing.
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

A path containing `:` cannot be expressed through the environment form. The
repeatable flag is the escape hatch for the operator's own invocation, but it
is not a complete one: the relay serializes the list into that same form to
stamp it onto each coder-backed member, so a layer holding `:` is a load-time
fault for any bundle with a member to stamp. A layer that is not valid Unicode
faults the same way and for the same reason — the environment carries text, and
substituting characters for undecodable bytes would stamp a path naming a
different directory than the relay read. Both remain usable where nothing needs
the representation: a bundle whose members are all coder-less, or a member
declaring its own `AGENTMUX_CONFIGURATION_DIRECTORY`. An empty element is
rejected in either form rather than read as the working directory.

The list is normalized to absolute paths after resolution, as the state root is
and for the same reason: it is stamped into every coder-backed member's
environment, and a relative layer re-resolves against each child's working
directory. Normalization is lexical, so a symlinked layer stays the path the
operator named rather than becoming its target.

Starter hydration applies only to a list from tier 3, which is a single layer. A
list supplied by flag or environment is never scaffolded; when one of its layers
does not exist, that is a fault rather than a reason to create one.

## State Root

The **state** root resolves by the same shape as the configuration root, from
`--state-directory`, then `AGENTMUX_STATE_DIRECTORY`, then
`$XDG_STATE_HOME/agentmux`, then `~/.local/state/agentmux`. It does not vary by
build profile either: nothing infers a deployment from where a process was
launched or how it was compiled.

One state root is one relay. The relay socket, both locks, the ready sentinel,
the principal store, and peer credentials all sit at the root rather than under
a bundle, so nothing below it separates two deployments. Isolation is expressed
by naming a distinct root and by nothing else. The **inscriptions** root
defaults to `<state_root>/inscriptions` and so follows it unless named.

The root is normalized to a non-empty absolute path before anything uses it.
That is a precondition for propagation rather than tidiness: the root is stamped
into every spawned member's environment, and a relative value would re-resolve
against each child's working directory. An empty `--state-directory` is rejected
instead of normalized, because the environment tier reads blank as absent and
one spelling of "nothing" must not mean two things.

### Propagation and the one authoritative stamp

The relay injects its normalized root as `AGENTMUX_STATE_DIRECTORY` when it
spawns a member, and that injection **overwrites** any value merged from coder,
bundle, or member configuration. It is the single exception to the
upsert-if-absent rule the rest of the bring-up context follows
(`BringUpContext`), on two grounds: the value is not known at configuration
load, since it belongs to the relay doing the spawn; and a declared value would
not override a preference but break the rendezvous, pointing the child at a
relay that never started it.

Both bring-up paths — first startup and `up`/reconcile — run through
`members_for_spawn`, so the path an operator happens to take cannot decide
whether a child can find its relay.

`BringUpContext::VARIABLE_NAMES` therefore stays the *load-time stamped* set —
bundle, session, and the configuration layer list.
`INHERITED_CONTEXT_VARIABLE_NAMES` is the wider one, adding the state root
because it is injected at spawn rather than stamped at load, and is what a
consumer sanitizing inherited context wants.

Every one of these names is defined together in `configuration::types`, and both
sets are derived from those definitions. That is not tidiness: a name held
anywhere else is a name both sets omit, with no consumer failing to say so —
which is how the configuration layer list went unstamped and unsanitized while
each list looked complete.

### Socket addressing

`sockaddr_un.sun_path` is 108 bytes on Linux (107 usable) and 104 on Darwin
(103 usable), and `<state_root>/bundles/<bundle>/tmux.sock` is the deepest path
the project builds. Normalizing the state root removed the relative-path escape
hatch that kept deep hierarchies under the limit, so `runtime::sockets`
reconstructs a short address instead: the parent directory is opened and the
socket addressed through that descriptor as `/proc/self/fd/<n>/<name>`. The
state root still has to be absolute for propagation, but the string handed to
`bind` or `connect` is a different string and need not be the same one.

`/proc` is Linux-only. On Darwin the descriptor form does not resolve and the
full path is used, which is what every caller did before — macOS keeps its
existing reach and does not gain depth-independence, but a path over the limit
reports the limit and the offending path rather than a bare `ENAMETOOLONG`.
Closing the gap would need a working-directory change, which is process-global;
Darwin offers no `bindat`/`connectat`. The limit is therefore a per-target
constant, since Linux's number would admit Darwin paths the kernel rejects.

Windows is an intended target, but this module does not reach it yet: it is
written against `std::os::unix::net`, and `std` exposes no AF_UNIX types on
Windows despite the OS supporting the family. A port needs a third-party
implementation, a Windows arm here (no `/proc`, so the full path is used, and
`sun_path` is 108 bytes there — Linux's figure, not the Darwin one the fallback
arm currently carries), and the same for the other Unix-typed surfaces in the
relay.

tmux binds its own socket from the `-S` argument it is given, so the descriptor
trick is unavailable there; the tmux client is run from the socket's directory
with a bare `-S tmux.sock`. That makes `-c` mandatory on session creation, since
tmux takes an omitted start directory from the client's working directory.

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
