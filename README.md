# agentmux

`agentmux` is a tmux-backed coordination runtime for multi-agent coding.
It provides one CLI (`agentmux`) with host + operator subcommands and one MCP
tool surface for LLM clients.

## Architecture At A Glance

- Relay host:
  - Command: `agentmux host relay [--no-autostart]`
  - Responsibility: start one relay process that serves configured bundles and route
    envelopes to tmux panes.
- MCP host:
  - Command: `agentmux host mcp`
  - Responsibility: expose MCP tools (`list`, `look`, `send`) and forward requests to relay.
- Operator CLI:
  - Commands: `agentmux list`, `agentmux look`, `agentmux send`
  - Responsibility: direct local inspection of recipients and message delivery.

Both host modes use shared runtime roots for configuration, sockets, locks, and
logs.

## Requirements

- `tmux` on `PATH`

## Install

```bash
cargo install agentmux
```

## Quick Start

1. Start relay for your bundle:

```bash
agentmux host relay
```

Optional: start relay processes without autostarting bundle runtimes:

```bash
agentmux host relay --no-autostart
```

Use lifecycle commands for explicit bundle transitions:

```bash
agentmux up myproject
agentmux down myproject
```

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

## Auto Start On Login

### Systemd (--user)

If you run `agentmux` as a user service, add:

`~/.config/systemd/user/agentmux-relay.service`

```ini
[Unit]
Description=agentmux relay host
After=default.target

[Service]
Type=simple
ExecStart=/usr/bin/env agentmux host relay
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
```

Enable and start the service:

```bash
systemctl --user daemon-reload
systemctl --user enable --now agentmux-relay.service
systemctl --user status agentmux-relay.service
```

Follow relay logs:

```bash
journalctl --user -u agentmux-relay.service -f
```

If coder CLIs are installed via tools like Mise/Asdf/NVM or a custom npm
prefix, set explicit environment in the unit so relay autostart can find
`codex`/`claude`/`opencode` consistently:

```ini
[Service]
Environment=PATH=/path/to/node/bin:/path/to/cargo/bin:/path/to/npm-prefix/bin:/usr/local/bin:/usr/bin:/bin
Environment=CODEX_HOME=/path/to/codex/home
Environment=CLAUDE_CONFIG_DIR=/path/to/claude/config
```

After environment changes, run `systemctl --user daemon-reload` and restart the
service.

## CLI Surface

```text
agentmux host relay [--no-autostart]
agentmux host mcp [--bundle NAME] [--session-name NAME]
agentmux up (<bundle-id> | --group GROUP)
agentmux down (<bundle-id> | --group GROUP)
agentmux list [--bundle NAME] [--sender NAME] [--json]
agentmux look <target-session> [--bundle NAME] [--lines N]
agentmux tui [--bundle NAME] [--sender NAME] [--lines N]
agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--bundle NAME] [--sender NAME] [--json]
```

Use `--help` on each command for the full flag list.

Bare `agentmux` dispatch behavior:

- interactive TTY: starts `agentmux tui`
- non-interactive context: prints help and exits non-zero

Common runtime flags for all commands:

- `--config-directory PATH`
- `--state-directory PATH`
- `--inscriptions-directory PATH` (alias: `--logs-directory PATH`)
- `--repository-root PATH`

`send` message input rules:

- Use `--message TEXT`, or pipe stdin.
- Do not provide both `--message` and piped stdin.
- Empty messages are rejected.

Example piped send:

```bash
printf 'queued hello\n' | agentmux send --bundle myproject --sender master --target tui
```

## MCP Surface

The MCP server advertises:

- `list`: return candidate recipients in the selected bundle.
- `look`: capture a read-only pane snapshot from a target session.
- `send`: deliver to explicit targets or broadcast.

Delivery behavior:

- `delivery_mode=async` (default): accept immediately and queue background delivery.
- `delivery_mode=sync`: block until per-target outcomes are known.
- `quiescence_timeout_ms` optionally bounds tmux prompt-readiness waiting.
- `acp_turn_timeout_ms` optionally bounds ACP turn-wait behavior.
- For ACP sync sends, success means first activity was observed. Terminal completion
  is correlated out-of-band by `message_id`.

Example `.mcp.json` snippet:

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

## Multi-Worktree Workflow

Typical topology:

- one shared bundle id (for example `agentmux`),
- one relay host process serving all configured bundle sockets (started by `agentmux host relay`),
- one MCP host per worktree/session identity (`master`, `relay`, `mcp`, `tui`).

Association resolution for `list`/`send` and MCP host startup:

- CLI flags have highest precedence (`--bundle`, `--sender` / `--session-name`).
- explicit CLI `--sender` values must match a configured bundle session; invalid
  values fail fast with `validation_unknown_sender` (no silent remap/fallback).
- Auto-discovery fallback:
  - bundle from Git common-dir owner name,
  - session from worktree top-level directory name.

TUI sender identity resolution:

- `--sender` flag
- local debug/testing override sender file
- `<config-root>/tui.toml` sender
- association fallback

## Configuration

Runtime roots by default:

- config root: `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`
- state root: `$XDG_STATE_HOME/agentmux` or `~/.local/state/agentmux`
- inscriptions (logs) root:
  - release: `<state-root>/inscriptions`
  - debug with `--repository-root` available: `<repo>/.auxiliary/inscriptions/agentmux`

Bundle configuration file path:

- `<config-root>/bundles/<bundle-name>.toml`

Optional TUI sender defaults:

- normal config file: `<config-root>/tui.toml`
- local debug/testing override:
  `.auxiliary/configuration/agentmux/overrides/tui.toml`

Starter files are generated when missing:

- `<config-root>/coders.toml`
- `<config-root>/bundles/example.toml`
- `<config-root>/policies.toml`

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

## Runtime Artifacts

Per-bundle state directory:

- `<state-root>/bundles/<bundle-name>/`

Important files:

- `relay.sock`: relay Unix socket
- `tmux.sock`: bundle tmux socket
- `relay.lock`: relay host lock
- `relay.spawn.lock`: relay spawn lock

Inscriptions:

- relay log: `<inscriptions-root>/bundles/<bundle-name>/relay.log`
- MCP log: `<inscriptions-root>/bundles/<bundle-name>/sessions/<session-name>/mcp.log`

## Troubleshooting

| Symptom | Likely Cause | Quick Check |
|---|---|---|
| `relay_unavailable` from CLI/MCP | Relay host is not running for selected bundle | Start relay: `agentmux host relay` |
| `relay_timeout` from CLI/MCP/TUI | Relay is running but request did not complete before timeout (often saturation/stuck workers) | Check relay health/logs and active stream clients; retry after relay restart |
| `authorization_forbidden` on `look` | Policy scope disallows non-self inspection | Check `<config-root>/policies.toml` `look` control |
| Sync ACP send reports timeout | First ACP activity not observed before timeout budget | Retry with async mode for coordination flow; inspect relay log by `message_id` |

## Planned Features

- Bundle/session `about` surfaces with human-readable descriptions for operators
  and agents.
- Mailbox-style message retrieval (`fetch`) and optional hold/quiet delivery
  mode to reduce coordination noise.
- Direct raw-write command support for CLI/TUI so users and agents can interact
  with coder sessions without dropping to tmux.
- Config include/pointer support so centrally hosted configs can reference
  project-local bundle definitions.
- Profile-based sender selection for CLI/TUI workflows.
- Additional autostart examples beyond systemd (for example launchd/OpenRC/Windows service patterns).

## Development

For local source development, install a Rust toolchain.

Validation commands:

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Source map:

- [src/README.md](src/README.md)
- [src/bin/README.md](src/bin/README.md)
- [src/runtime/README.md](src/runtime/README.md)
- [src/mcp/README.md](src/mcp/README.md)
- `documentation/architecture/openspec/specs/`

## License

[Apache 2.0](LICENSE)
