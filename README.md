# agentmux

`agentmux` is a product-agnostic runtime for inter-agent communication that
lets agent sessions exchange structured messages and coordinate work without
being tied to one specific coding product or harness. It supports agent
harnesses running in tmux panes and ACP-backed sessions.

## Disclaimer

This project is **not affiliated with** [agentmux.app](https://agentmux.app/)
in any way.

## Documentation

- Usage guides: [documentation/usage/README.md](documentation/usage/README.md)
- Tool comparisons: [documentation/comparisons.md](documentation/comparisons.md)
- Developer guide: [documentation/development/README.md](documentation/development/README.md)

## Requirements

- `tmux` on `PATH`
- `Zig 0.15.x` on `PATH` (only required for the `pty` Cargo feature;
  default `cargo build` does not invoke Zig)
- For libghostty-vt's vendored ghostty clone (only when building
  with `--features pty` without a local override): outbound network
  access to `github.com/ghostty-org/ghostty.git`. To bypass the network
  clone, set `GHOSTTY_SOURCE_DIR` to a pre-checked-out ghostty source
  tree that contains `build.zig`; to bypass Zig's package fetch, set
  `GHOSTTY_ZIG_SYSTEM_DIR` to a directory containing the Zig package
  cache. The `libghostty-vt-sys/pkg-config` feature (skip Zig entirely
  via an installed `libghostty-vt`) is not currently reachable
  through agentmux's consumer dep — see
  `documentation/development/README.md` Zig-free Pty Builds.

## Install

```bash
cargo install agentmux
```

## Quick Start

1. Start relay for your bundle:

```bash
agentmux host relay
```

Optional: start relay without autostarting bundle runtimes:

```bash
agentmux host relay --no-autostart
```

By default the relay watches the bundles configuration directory and reconciles
running bundles when their files change: a new bundle file is loaded and
started, a removed file unloads its bundle, and a **modified file triggers a
full teardown** of that bundle — every active session in it is disconnected and
the bundle's runtime is torn down and restarted with the new configuration.
That is a sharp edge: editing a live bundle file cuts off its agents mid-
operation. Edit bundle files only when that bundle is idle, or start the relay
with `--no-watch` to disable runtime reconciliation entirely (changes then take
effect only on the next relay restart):

```bash
agentmux host relay --no-watch
```

Relay-wide controls have a durable home in `<config-root>/relay.toml`, whose keys
live at the file root (kebab-case):

```toml
watch-bundles = true               # default true; disable to freeze the bundle set
require-session-credentials = false # default false; true rejects socket-trust Hellos

[choices]
pending-max = 256                  # bounded choices queue depth

# Active outbound peer relays. A cross-relay target (<session>@<bundle>!<alias>)
# is forwarded to the peer you locally call <alias>. Each entry names: `alias`,
# this relay's LOCAL name for the peer (the !<alias> routing selector, internal to
# us); `address`, the peer's listening Unix-domain socket (an absolute path; a
# host:port TCP endpoint is not yet supported); and `connect-as`, the identity the
# peer issued this relay (presented as <connect-as>@RELAY when dialing it, since
# the receiver determines it). Inbound scope for a connecting peer is set
# separately via `new peer <id>@RELAY --scope`, not in this file.
[[peers]]
alias = "west"
address = "/run/agentmux/west.sock"
connect-as = "east"
```

`watch-bundles` and `require-session-credentials` resolve by precedence: CLI
override (`--no-watch`, `--require-credentials`) > environment override
(`AGENTMUX_RELAY_WATCH_BUNDLES`, `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`,
each exactly `true` or `false`) > `relay.toml` > defaults. A malformed
`relay.toml`, unknown field, wrong type, or invalid peer entry fails relay
startup and `agentmux check configuration` with a structured error.

2. Start MCP host:

```bash
agentmux host mcp
```

3. Add MCP server wiring in `.mcp.json` (or equivalent MCP config):

```json
{
  "mcpServers": {
    "agentmux": {
      "command": "agentmux",
      "args": ["host", "mcp"]
    }
  }
}
```

4. Use lifecycle commands for explicit bundle transitions:

```bash
agentmux up myproject
agentmux down myproject
```

For login-time startup, service examples, shared runtime flags, and runtime
artifact locations, see
[documentation/usage/operations.md](documentation/usage/operations.md).

## Development Prerequisites

The pre-commit hooks and CI use [cargo-nextest](https://nexte.st/) as the
test runner. Install it locally if you intend to run the test suite:

```bash
cargo install cargo-nextest --locked
```

The hooks will fail otherwise. See
[documentation/development-practices.md](documentation/development-practices.md)
for the full development workflow.

## Architecture At A Glance

- Relay host:
  - Command: `agentmux host relay [--no-autostart] [--no-watch]`
  - Responsibility: start one relay process that serves configured bundles
    ("agent teams") and routes envelopes to target runtimes. Watches bundle
    config files for runtime add/remove/modify unless `--no-watch` is set.
- MCP host:
  - Command: `agentmux host mcp`
  - Responsibility: expose MCP tools (`list`, `help`, `look`, `choose`,
    `updown`, `new`, `change`, `raww`, `send`) and forward requests to relay.
- Operator CLI:
  - Commands: `agentmux list principals`, `agentmux look`, `agentmux raww`,
    `agentmux send`, `agentmux tui`
  - Responsibility: direct local inspection, message delivery, and interactive
    coordination flows with relay auto-start fallback for `agentmux tui`.

Both host modes use shared runtime roots for configuration, sockets, locks, and
logs.

## CLI Surface

```text
agentmux host relay [--no-autostart] [--require-credentials] [--no-watch]
agentmux host mcp [--bundle NAME] [--session-name NAME]
agentmux up (<bundle-id> | --group GROUP)
agentmux down (<bundle-id> | --group GROUP)
agentmux list principals [--namespace NAME|GLOBAL|*] [--as-session NAME] [--json]
agentmux look <target-session> [--bundle NAME] [--as-session NAME] [--lines N]
agentmux raww <target-session> --text TEXT [--no-enter] [--bundle NAME] [--as-session NAME] [--json]
agentmux new peer <principal-id> [--scope SCOPE] [--output PATH] [--bundle NAME] [--as-session NAME] [--json]
agentmux change psk <principal-id> [--bundle NAME] [--as-session NAME] [--json]
agentmux check configuration [<bundle-id>]
agentmux tui [--bundle NAME] [--as-session NAME] [--lines N]
agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--request-id ID] [--bundle NAME] [--as-session NAME] [--json]
```

Use `--help` on each command for the full flag list.

Bare `agentmux` dispatch behavior:

- interactive TTY: starts `agentmux tui`
- non-interactive context: prints help and exits non-zero

For shared runtime flags and operational details, see
[documentation/usage/operations.md](documentation/usage/operations.md).

## MCP Surface

The MCP server advertises:

- `help`: return tool/command help and JSON argument schemas.
- `list`: meta-tool for relay discovery; requires a `command` — `principals`
  (session listing, with `args.namespace` scoping the lookup), `namespaces`,
  `relays`, or `decisions` (the bundle's pending ACP choice queue).
- `look`: capture a read-only session snapshot from a target session.
- `choose`: submit an ACP-native choice decision; gated on the `choose` policy
  capability.
- `updown`: administer the associated bundle's runtime (`command="up"` /
  `command="down"`).
- `new`: register a peer principal and mint its PSK (`command="peer"`).
- `change`: rotate a principal's PSK (`command="psk"`).
- `raww`: write raw text directly to one target session.
- `send`: deliver to explicit targets or broadcast.

See [src/mcp/README.md](src/mcp/README.md) for detailed MCP command semantics.

Delivery behavior:

- ACP delivery bounds are configured per-coder under
  `[coders.<id>.acp]` (`prime-timeout-ms`); v1 has no per-call
  operator override.
- Tmux delivery bounds are configured per-coder under
  `[coders.<id>.tmux]` (`prime-timeout-ms`, `wedge-detection`); v1
  has no per-call operator override.
- Pty sessions use the same look bounds as Tmux (the relay truncates
  to `mode.lines` rows). Pty delivery bounds are configured per-coder
  under `[coders.<id>.pty]` (`prime-timeout-ms`, `wedge-detection`,
  `cols`, `rows`); v1 has no per-call operator override.
- Terminal completion is correlated out-of-band by `message_id`.

## Multi-Worktree Workflow

Typical topology:

- one shared bundle id (for example `agentmux`),
- one relay host process serving all configured bundles through a single socket,
- one MCP host per worktree/session identity (`master`, `relay`, `mcp`, `tui`).

Association resolution:

- `list principals` and `host mcp` use association auto-discovery fallback:
  - bundle from Git common-dir owner name,
  - session from worktree top-level directory name,
- `send` and `tui` use global session/surface selectors:
  - `--bundle`, else `default-bundle` from `ui.toml`,
  - `--as-session`, else `default-session` from `users.toml`,
  - one-shot `send` fails fast when the bundle is missing or unknown;
    interactive `tui` falls back to the first available bundle (or an empty
    browsing context).

TUI session identity resolution:

- `--as-session` selector
- `default-session` from active `users.toml`
- no association fallback for TUI/send

## Configuration

Runtime roots by default:

- config root: `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`
- state root: `$XDG_STATE_HOME/agentmux` or `~/.local/state/agentmux`
- inscriptions root: `<state-root>/inscriptions`

Bundle configuration file path:

- `<config-root>/bundles/<bundle-name>.toml`

Global user and authorization configuration:

- `<config-root>/users.toml`: maps session identities to policy presets
- `<config-root>/policies.toml`: defines policy presets with per-control scopes

Per-control scopes form a ladder — `self` (act only on yourself), `home`
(act on any principal in your own/home namespace), `all` (act across
namespaces). A principal's *home* is its native namespace: a session's home is
its bundle, and a relay-wide principal (such as a `@GLOBAL` operator) lives in
its reserved namespace (`GLOBAL`/`EXTERNAL`/`RELAY`). Reaching *into* a bundle
you do not live in requires `all`, so a `@GLOBAL` operator needs `all` to
list or message a bundle's sessions. Messaging the operator is the common case
that does not: a relay-wide (`@GLOBAL`) target is always reachable from any
bundle under `home`, so an agent can reply to the operator without
cross-namespace scope.

Global TUI session configuration:

- identity/policy file: `<config-root>/users.toml`
  - `default-session`
  - `[[sessions]]` with `id`, optional `name`, and `policy`
- UI-surface file: `<config-root>/ui.toml`
  - `default-bundle`

Starter files are generated when missing:

- `<config-root>/coders.toml`
- `<config-root>/bundles/example.toml`
- `<config-root>/policies.toml`
- `<config-root>/users.toml`
- `<config-root>/ui.toml`
- `<config-root>/relay.toml`

### Example `coders.toml`

```toml
format-version = 1

[[coders]]
id = "codex"

[coders.tmux]
initial-command = "codex"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "(?m)^›"
prompt-inspect-lines = 3
prompt-idle-column = 2

[[coders]]
id = "opencode"

[coders.acp]
channel = "stdio"
command = "opencode acp"

# Pty transport (libghostty-vt-backed delivery with portable-pty child
# process management). Opt-in per-coder; the pty Cargo feature must be
# enabled at build time (`cargo build --features pty`). Each coder
# entry declares exactly one of `tmux`, `acp`, or `pty`; the validator
# rejects both or neither.
#
# Per-coder `term-protocol` selects the literal `TERM` env-var value
# the Pty transport sets when spawning the child. Defaults to
# `xterm-256color`. Supported values:
#
# - xterm-256color (default): modern xterm with 256 colors.
# - xterm-kitty:              advertises kitty-keyboard-protocol
#                              support to TUIs that honor it. The
#                              project's TUIs (claude, codex, gemini,
#                              opencode) detect terminal capabilities
#                              from TERM and adapt accordingly; see
#                              [Claude Code terminal-config](https://code.claude.com/docs/en/terminal-config.md)
#                              and the [opencode config docs](https://opencode.ai/docs)
#                              for the per-TUI detection patterns.
#                              Whether TERM is sufficient on its own
#                              to flip a TUI into kitty-protocol mode
#                              depends on the TUI's libghostty-vt /
#                              terminal emulation stack; the live
#                              joint smoke session (`tasks.md` §6.6)
#                              is the empirical ground truth for the
#                              project's TUIs.
# - alacritty, foot, wezterm, screen-256color: pass-through to the
#   child for emulator-specific behavior.
[[coders]]
id = "codex-pty"

[coders.pty]
initial-command = "codex"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "(?m)^READY_MARKER"
prompt-inspect-lines = 3
cols = 120
rows = 40
prime-timeout-ms = 30000
wedge-detection = true
term-protocol = "xterm-kitty"
```

### Example `bundles/myproject.toml`

```toml
format-version = 1
groups = ["dev", "login"]

[[sessions]]
id = "master"
name = "GPT (Coordinator)"
directory = "/home/me/src/myproject"
coder = "codex"
coder-session-id = "00000000-0000-0000-0000-000000000000"

[[sessions]]
id = "tui"
name = "GPT (Frontend Engineer)"
directory = "/home/me/src/WORKTREES/myproject/tui"
coder = "codex"
```

## Planned Features

- Bundle/session `about` surfaces with human-readable descriptions for operators
  and agents.
- Mailbox-style message retrieval (`fetch`) and optional hold/quiet delivery
  mode to reduce coordination noise.
- Direct raw-write command support for CLI/TUI so users and agents can interact
  with coder sessions without dropping to tmux.
- Config include/pointer support so centrally hosted configs can reference
  project-local bundle definitions.
- Expanded global TUI session-management ergonomics (session lifecycle and
  keybinding customization).
- Additional autostart examples beyond systemd (for example
  launchd/OpenRC/Windows service patterns).
- Native Windows support (direct PTY/ConPTY and non-tmux transport path).

## License

[Apache 2.0](LICENSE)
