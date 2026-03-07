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
MCP startup does not require relay availability.
If relay is down, MCP `list` and `chat` return structured `relay_unavailable`
errors until relay is reachable.
Relay startup performs a reconciliation pass that ensures configured bundle
sessions are present on the bundle tmux socket.
Relay tmux operations use the bundle runtime socket path:
`$STATE_ROOT/bundles/<bundle-name>/tmux.sock`.

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
`validation_empty_targets`, `validation_unknown_sender`,
`relay_unavailable`).

## Pane Envelope Format (MVP)

Relay pane injection now renders a structured envelope with:

1. Compact single-line JSON manifest preamble.
2. RFC 822-style headers (`Envelope-Version`, `Message-Id`, `Date`, `From`,
   `To`, optional `Cc`, optional `Subject`, and `Content-Type`).
3. `multipart/mixed` MIME body with:
   - required `text/plain; charset=utf-8` chat body part
   - reserved `application/vnd.tmuxmux.path-pointer+json` extension part
4. Closing MIME boundary `--<boundary>--` as end marker.

Rendered addresses use:

- `Display Name <session:session_name>`

Canonical routing remains driven by manifest `target_sessions`; `Cc` is
informational only.

## Prompt Batching

Envelope prompts are batched under a token budget with default:

- `max_prompt_tokens = 4096`
- tokenizer profile default `characters_0_point_3`

Configuration environment variables:

- `TMUXMUX_MAX_PROMPT_TOKENS` (positive integer)
- `TMUXMUX_TOKENIZER_PROFILE` (`characters_0_point_3`, `whitespace_rough`)

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

## Prompt-Readiness Templates

Quiescence can still occur when a session is not at an input-ready prompt.
Coder definitions may define an optional prompt-readiness template that must
match before relay injection.

Configuration fields:

- `prompt-regex` (optional): regular expression evaluated against inspected
  pane tail text. Multi-line matching is supported (for example `(?m)^›`).
- `prompt-inspect-lines` (optional): tail lines to inspect after trimming only
  trailing blank lines from pane capture output (interior blank lines are
  preserved). Default is `3`;
  effective range is clamped to `1..=40`.
- `prompt-idle-column` (optional): required tmux `cursor_x` value for
  input-idle delivery. Use this to avoid injecting while a user is typing.

If a session reaches quiescence but prompt regex never matches before
`delivery_timeout_ms`, relay reports a timeout reason indicating prompt
readiness mismatch and does not inject that message.

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

Configuration paths:

- `<config-root>/coders.toml`
- `<config-root>/bundles/<bundle-name>.toml`

Session fields:

- `id`: canonical routing identity (tmux session target).
- `name` (optional): human-readable recipient label; chat targets may use
  either `id` or `name`.

Default config root:

- debug builds:
  - repository-local `.auxiliary/configuration/tmuxmux/` when present
- otherwise:
  - `$XDG_CONFIG_HOME/tmuxmux` or `~/.config/tmuxmux`

Example `coders.toml`:

```toml
format-version = 1

[[coders]]
id = "codex"
initial-command = "codex"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "(?m)^›"
prompt-inspect-lines = 3
prompt-idle-column = 3
```

Example `bundles/tmuxmux.toml`:

```toml
format-version = 1

[[sessions]]
id = "relay"
name = "Relay"
directory = "/home/me/src/WORKTREES/tmuxmux/relay"
coder = "codex"
coder-session-id = "00000000-0000-0000-0000-000000000000"

[[sessions]]
id = "tui"
name = "TUI"
directory = "/home/me/src/WORKTREES/tmuxmux/tui"
coder = "codex"
```

Per-worktree local MCP overrides (Git-ignored) may be placed at:

- `.auxiliary/configuration/tmuxmux/overrides/mcp.toml`

See runtime bootstrap spec for full details:
[runtime-bootstrap spec](documentation/architecture/openspec/specs/runtime-bootstrap/spec.md).

## Smoke Tests

Manual prompt-readiness smoke procedure:

- [tests/smoke/prompt_readiness_manual.md](tests/smoke/prompt_readiness_manual.md)

## License

[Apache 2.0](LICENSE)
