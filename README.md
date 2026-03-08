# agentmux

`agentmux` is a tmux-backed coordination layer for agentic coding sessions.

It provides two runtime hosts:

- relay host: manages bundle sessions and routes messages into tmux panes
- MCP host: exposes MCP tools (`list`, `send`) for LLM agents

## Motivation

Some coding agents support hooks for coordination, while others do not.
`agentmux` uses tmux session control as a common denominator so different
agent harnesses can exchange messages through a shared transport.

## Requirements

- `tmux` on `PATH`

## Quick Start

Install `agentmux`:

```bash
cargo install --path .
```

Start relay host:

```bash
agentmux host relay myproject
```

Start MCP host:

```bash
agentmux host mcp --bundle myproject --session-name master
```

Configure MCP client integration (for example, in `.mcp.json`):

```json
{
  "mcpServers": {
    "agentmux": {
      "command": "agentmux",
      "args": [
        "host",
        "mcp",
        "--bundle",
        "myproject",
        "--session-name",
        "master"
      ]
    }
  }
}
```

Run relay/MCP with shared state root:

```bash
AGENTMUX_STATE_DIRECTORY=.auxiliary/state/myproject
agentmux host relay myproject --state-directory "${AGENTMUX_STATE_DIRECTORY}"
agentmux host mcp --bundle myproject --session-name master --state-directory "${AGENTMUX_STATE_DIRECTORY}"
```

## Configuration

By default:

- Config root: `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`
- State root: `$XDG_STATE_HOME/agentmux` or `~/.local/state/agentmux`

In debug builds, repository-local defaults are used when available under
`.auxiliary/configuration/agentmux` and `.auxiliary/state/agentmux`.

Starter files are created when missing:

- `<config-root>/coders.toml`
- `<config-root>/bundles/example.toml`

Bundle config files live at:

- `<config-root>/bundles/<bundle-name>.toml`

### Example `coders.toml`

```toml
format-version = 1

[[coders]]
id = "codex"
initial-command = "codex"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "(?m)^›"
prompt-inspect-lines = 3
prompt-idle-column = 2
```

### Example `bundles/myproject.toml`

```toml
format-version = 1

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

## Runtime Notes

- Start relay host before MCP host for normal operation.
- MCP startup does not require relay to already be reachable.
- If relay is unavailable, MCP tools return structured `relay_unavailable`
  errors.
- Relay reconciliation ensures configured sessions exist on the bundle tmux
  socket.

## Delivery Behavior

MCP `send` supports:

- `delivery_mode=async` (default): accept immediately, deliver in background.
- `delivery_mode=sync`: block until per-target delivery outcomes are known.

Optional `quiescence_timeout_ms` bounds how long delivery waits for prompt
quiescence.

## Logging (Inscriptions)

Default inscriptions root:

- Debug builds: `<repository-root>/.auxiliary/inscriptions/agentmux`
- Release builds: `<state-root>/inscriptions`

Per-bundle relay log:

- `<inscriptions-root>/bundles/<bundle-name>/relay.log`

Per-session MCP log:

- `<inscriptions-root>/bundles/<bundle-name>/sessions/<session-name>/mcp.log`

## Development

Validation commands:

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Architecture and implementation notes:

- `src/README.md` and subsystem README files under `src/`
- `documentation/architecture/openspec/specs/`

## License

[Apache 2.0](LICENSE)
