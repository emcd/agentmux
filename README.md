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

## Architecture At A Glance

- Relay host:
  - Command: `agentmux host relay [--no-autostart]`
  - Responsibility: start one relay process that serves configured bundles
    ("agent teams") and routes envelopes to target runtimes.
- MCP host:
  - Command: `agentmux host mcp`
  - Responsibility: expose MCP tools (`list`, `look`, `send`) and forward
    requests to relay.
- Operator CLI:
  - Commands: `agentmux list`, `agentmux look`, `agentmux send`, `agentmux tui`
  - Responsibility: direct local inspection, message delivery, and interactive
    coordination flows with relay auto-start fallback for `agentmux tui`.

Both host modes use shared runtime roots for configuration, sockets, locks, and
logs.

## CLI Surface

```text
agentmux host relay [--no-autostart]
agentmux host mcp [--bundle NAME] [--session-name NAME]
agentmux up (<bundle-id> | --group GROUP)
agentmux down (<bundle-id> | --group GROUP)
agentmux list [--bundle NAME] [--sender NAME] [--json]
agentmux look <target-session> [--bundle NAME] [--lines N]
agentmux tui [--bundle NAME] [--session NAME] [--lines N]
agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--delivery-mode async|sync] [--quiescence-timeout-ms MS] [--acp-turn-timeout-ms MS] [--request-id ID] [--bundle NAME] [--session NAME] [--json]
```

Use `--help` on each command for the full flag list.

Bare `agentmux` dispatch behavior:

- interactive TTY: starts `agentmux tui`
- non-interactive context: prints help and exits non-zero

For shared runtime flags and operational details, see
[documentation/usage/operations.md](documentation/usage/operations.md).

## MCP Surface

The MCP server advertises:

- `list`: return candidate recipients in the selected bundle.
- `look`: capture a read-only pane snapshot from a target session.
- `send`: deliver to explicit targets or broadcast.

Delivery behavior:

- `delivery_mode=async` (default): accept immediately and queue background
  delivery.
- `delivery_mode=sync`: block until per-target sync outcomes are known.
- `quiescence_timeout_ms` optionally bounds tmux prompt-readiness waiting.
- `acp_turn_timeout_ms` optionally bounds ACP turn-wait behavior.
- For ACP sync sends, success is declared at first observed ACP activity
  (`details.delivery_phase = accepted_in_progress`); relay does not wait for
  terminal turn completion before returning sync success.
- Terminal completion is correlated out-of-band by `message_id`.

## Multi-Worktree Workflow

Typical topology:

- one shared bundle id (for example `agentmux`),
- one relay host process serving all configured bundle sockets,
- one MCP host per worktree/session identity (`master`, `relay`, `mcp`, `tui`).

Association resolution:

- `list` and `host mcp` use association auto-discovery fallback:
  - bundle from Git common-dir owner name,
  - session from worktree top-level directory name,
- `send` and `tui` use global TUI session selectors:
  - `--bundle` or `default-bundle`,
  - `--session` or `default-session`,
  - fail-fast validation when selectors are missing or unknown.

TUI session identity resolution:

- `--session` selector
- active `tui.toml` defaults (`default-session`)
- no association fallback for TUI/send

## Configuration

Runtime roots by default:

- config root: `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`
- state root: `$XDG_STATE_HOME/agentmux` or `~/.local/state/agentmux`
- inscriptions root: `<state-root>/inscriptions`

Bundle configuration file path:

- `<config-root>/bundles/<bundle-name>.toml`

Global TUI session configuration:

- normal config file: `<config-root>/tui.toml`
- keys:
  - `default-bundle`
  - `default-session`
  - `[[sessions]]` with `id`, optional `name`, and `policy`

Starter files are generated when missing:

- `<config-root>/coders.toml`
- `<config-root>/bundles/example.toml`
- `<config-root>/policies.toml`
- `<config-root>/tui.toml`

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
