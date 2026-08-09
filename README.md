# agentmux

`agentmux` is a product-agnostic runtime for inter-agent communication that
lets agent sessions exchange structured messages and coordinate work without
being tied to one specific coding product or harness. It supports agent
harnesses running in tmux panes and ACP-backed sessions.

> **The Pty transport is work-in-progress and not production-ready.** It is not
> yet a supported way to run coder sessions in 0.9.0: known deferred gaps are
> `agentmux:issues/runtime/8` (lazy Pty spawn panics in a tokio worker) and
> `agentmux:issues/runtime/9` (a delivery can spawn a member of a held bundle),
> both targeted for the 0.10.0 cycle. Until they land, treat Pty-backed members
> as experimental.

## Disclaimer

This project is **not affiliated with** [agentmux.app](https://agentmux.app/)
in any way.

## Documentation

- Usage guides: [documentation/usage/README.md](documentation/usage/README.md)
  - [Maintainer Configuration Guide](
    documentation/usage/maintainer-configuration-guide.md) —
    file-by-file configuration root contents, layering, starter hydration
  - [Operations Guide](documentation/usage/operations.md) — runtime flags,
    service startup, runtime artifact locations
  - [Authorization](documentation/usage/authorization.md) — policy scopes,
    per-control ladder, home namespace, reachability rules
  - [CLI Surface](documentation/usage/cli-surface.md) — full command
    reference and shared runtime flags
  - [MCP Surface](documentation/usage/mcp-surface.md) — MCP tool inventory
    and delivery behavior
  - [Multi-Worktree Workflow](documentation/usage/multi-worktree-workflow.md)
    — topology and association resolution
  - [TUI Workbench Guide](documentation/usage/tui.md) — interactive TUI
    session identity, browsing, keybindings
- Tool comparisons:
  [documentation/comparisons.md](documentation/comparisons.md)
- Developer guide:
  [documentation/development/README.md](documentation/development/README.md)

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

Relay-wide controls have a durable home in `<config-root>/relay.toml`. The
`watch-bundles` and `require-session-credentials` keys, the
`[choices].pending-max` bound, and outbound peer entries are detailed in
[documentation/usage/operations.md](documentation/usage/operations.md)
and
[documentation/usage/maintainer-configuration-guide.md](
  documentation/usage/maintainer-configuration-guide.md).

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
[documentation/development-practices.md](
  documentation/development-practices.md)
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

For the full command reference, MCP tool inventory, multi-worktree topology
and association resolution, configuration root contents, and authorization
model, see the [usage guides](documentation/usage/README.md):

- [CLI Surface](documentation/usage/cli-surface.md) — `host`, `up`/`down`,
  `list`, `look`, `raww`, `send`, `tui`, `new`, `change`, `check`, and
  shared runtime flags.
- [MCP Surface](documentation/usage/mcp-surface.md) — `list` (with
  `principals`/`namespaces`/`relays`/`decisions`), `look`, `choose`,
  `updown`, `new`, `change`, `raww`, `send`, and per-coder delivery bounds.
- [Multi-Worktree Workflow](documentation/usage/multi-worktree-workflow.md)
  — typical topology, the MCP four-tier association precedence for
  `host mcp`, and the one-shot UI/user selector family used by
  `list principals` / `send` / `tui`.
- [Maintainer Configuration Guide](
  documentation/usage/maintainer-configuration-guide.md) —
  file-by-file configuration root contents, layering, starter hydration,
  and worked examples (minimal deployment, layered base + R&D variant,
  multi-session shared-worktree).
- [Authorization](documentation/usage/authorization.md) — policy scopes,
  per-control ladder (`self`/`home`/`all`), home namespace concept, and
  reachability rules including the `updown = "home"` requirement for
  bundle lifecycle.

## Known Security Limitations

This is alpha software under active development. The following gaps are
known and deliberately deferred past the 0.9.0 release rather than fixed
now:

- **Credential expiry/revocation does not terminate already-connected
  sessions.** Expiry and revocation are enforced only at connection time --
  the Hello handshake rejects an expired or revoked credential up front. A
  session that is already connected when its credential expires or is
  explicitly revoked keeps running until it happens to reconnect; there is no
  active teardown trigger for a live session.
- **No forced-takeover path for identity claims.** There is no
  operator-controlled mechanism to force a stale or compromised session off a
  claimed identity ahead of its own reconnect.
- **Credential configuration writes can follow symlinked ancestor
  directories.** `new peer --write-config` and `change psk --write-config`
  derive their destination beneath the state root, and the final write uses
  `O_NOFOLLOW`, but the directory-creation, permission-change, and rename
  steps leading up to it still follow symlinks in the ancestor path. A
  symlink placed beneath the state tree can redirect a credential write
  outside it. Avoid symlinks beneath your configured state root until this
  is fixed.

These are prioritized for the 0.10.0 release.

## Planned Features

- Bundle/session `about` surfaces with human-readable descriptions
  for operators and agents.
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
