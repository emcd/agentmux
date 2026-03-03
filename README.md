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

## Development

Validation commands:

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Pre-commit hooks are configured in
`.auxiliary/configuration/pre-commit.yaml`.

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

See runtime bootstrap spec for full details:
[runtime-bootstrap spec](documentation/architecture/openspec/changes/add-runtime-bootstrap-and-xdg-layout/specs/runtime-bootstrap/spec.md).

## License

[Apache 2.0](LICENSE)
