# tmuxmux

`tmuxmux` is a tmux-backed coordination layer for agentic coding sessions.

It provides:

- A relay process for routing chat envelopes between tmux sessions.
- An MCP server surface for LLM-facing tools (`list`, `chat` in MVP).
- A local-first runtime model with XDG-compliant configuration and state.

## Motivation

Some coding agents support hooks for coordination, while others do not. `tmuxmux`
uses tmux session control as a common denominator so different agent harnesses
can still exchange messages through a shared transport.

## Status

The project is in early implementation phase.

Design contracts are captured in OpenSpec proposals:

- `add-mcp-session-relay-mvp`
- `add-mcp-tool-surface-contract`
- `add-runtime-bootstrap-and-xdg-layout`
- `add-pane-envelope-rfc822-mime`

See [documentation/architecture/openspec](documentation/architecture/openspec).

## Quick Start

### Prerequisites

- Rust stable toolchain
- `tmux` available on `PATH`

### Build

```bash
cargo build
```

### Run binaries

Relay:

```bash
cargo run --bin tmuxmux-relay
```

MCP server:

```bash
cargo run --bin tmuxmux-mcp
```

Optional explicit association overrides:

```bash
cargo run --bin tmuxmux-mcp -- --bundle-name tmuxmux --session-name relay
```

## Recommended Startup Pattern

Start relay first, then MCP servers.

Use the same `--bundle` and `--state-directory` values for relay and MCP so
both resolve the same `relay.sock`.
MCP startup is connect-only and fails fast if the relay socket is unavailable.
Relay startup performs a reconciliation pass that ensures configured bundle
sessions are present on the bundle tmux socket.

Example:

```bash
cargo run --bin tmuxmux-relay -- --bundle tmuxmux --state-directory .auxiliary/state/tmuxmux
cargo run --bin tmuxmux-mcp -- --bundle-name tmuxmux --session-name relay --state-directory .auxiliary/state/tmuxmux
```

## MCP Tool Schemas (MVP)

### `list`

Arguments:

```json
{}
```

Success payload shape:

```json
{
  "schema_version": "1",
  "bundle_name": "tmuxmux",
  "recipients": [
    {"session_name": "codex-b", "display_name": "Codex B"}
  ]
}
```

### `chat`

Arguments (explicit targets):

```json
{
  "request_id": "req-1",
  "message": "Can you review the runtime tests?",
  "targets": ["codex-b"],
  "broadcast": false
}
```

Arguments (broadcast):

```json
{
  "request_id": "req-2",
  "message": "Standup in 5 minutes.",
  "targets": [],
  "broadcast": true
}
```

Success payload shape:

```json
{
  "schema_version": "1",
  "bundle_name": "tmuxmux",
  "request_id": "req-1",
  "sender_session": "codex-a",
  "sender_display_name": "Codex A",
  "status": "partial",
  "results": [
    {
      "target_session": "codex-b",
      "message_id": "4f5f2b40-8c25-4a95-8b7d-42aa6b0ab070",
      "outcome": "delivered"
    },
    {
      "target_session": "codex-c",
      "message_id": "9f4f6e22-913a-49f5-82e9-2215d24c3907",
      "outcome": "timeout",
      "reason": "delivery_quiescence_timeout"
    }
  ]
}
```

Validation and delivery failures return structured error codes in MCP error
data (for example `validation_conflicting_targets`,
`validation_empty_targets`, `validation_unknown_sender`).

## Development

Validation commands:

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

### Local Testing Tip

For relay/MCP smoke tests, use a shared shell variable to reduce
`--state-directory` mismatch mistakes:

```bash
TMUXMUX_STATE_DIRECTORY=.auxiliary/state/tmuxmux
cargo run --bin tmuxmux-relay -- --bundle tmuxmux --state-directory "${TMUXMUX_STATE_DIRECTORY}"
cargo run --bin tmuxmux-mcp -- --bundle-name tmuxmux --session-name relay --state-directory "${TMUXMUX_STATE_DIRECTORY}"
```

Pre-commit hooks are configured in
`.auxiliary/configuration/pre-commit.yaml`.

## Quiescence Delivery Notes

Relay delivery waits for pane output to remain stable before injecting a prompt.

Default values:

- `quiet_window_ms = 750`
- `delivery_timeout_ms = 30000`

If pane output changes continuously (for example, clock-like status output),
delivery may time out for that target.

## Planned Runtime Layout (MVP)

Configuration root:

- `$XDG_CONFIG_HOME/tmuxmux` or `~/.config/tmuxmux`

State root:

- `$XDG_STATE_HOME/tmuxmux` or `~/.local/state/tmuxmux`

Per-bundle sockets:

- `tmux.sock`
- `relay.sock`

## Bundle Configuration (MVP)

Bundle membership is operator-managed in MVP and is not mutated via MCP tools.

Configuration path:

- `$XDG_CONFIG_HOME/tmuxmux/bundles/<bundle-name>.json`
- fallback: `~/.config/tmuxmux/bundles/<bundle-name>.json`

Example:

```json
{
  "schema_version": "1",
  "members": [
    {
      "session_name": "codex-a",
      "display_name": "Codex A",
      "working_directory": "/home/me/src/tmuxmux",
      "start_command": "codex resume <uuid>"
    },
    {
      "session_name": "codex-b",
      "display_name": "Codex B"
    }
  ]
}
```

Per-worktree local MCP overrides (Git-ignored) may be placed at:

- `.auxiliary/configuration/tmuxmux/overrides/mcp.toml`

See runtime bootstrap spec for full details:
[runtime-bootstrap spec](documentation/architecture/openspec/changes/add-runtime-bootstrap-and-xdg-layout/specs/runtime-bootstrap/spec.md).

## License

[Apache 2.0](LICENSE)
