# Change: Add unified agentmux CLI with host/list/send commands

## Why

`agentmux` currently exposes separate binaries (`agentmux-relay`,
`agentmux-mcp`) and no first-class operator CLI for direct list/send
operations. This makes command discovery harder and leaves CLI verb style
inconsistent.

We want one primary command with short Germanic verbs, while preserving
existing binary entrypoints for compatibility.

## What Changes

- Add a primary `agentmux` command with subcommands:
  - `host relay <bundle-id>`
  - `host mcp`
  - `list`
  - `send`
- Prefer `host` over `serve`/`start` as the canonical verb for process hosting.
- Use positional bundle selection for relay hosting:
  - `agentmux host relay <bundle-id>`
- Define `send` message input resolution for MVP:
  - use `--message` when provided,
  - else read piped stdin when present,
  - else fail with a structured missing-message validation error.
- Keep `agentmux-relay` and `agentmux-mcp` as compatibility wrappers.
- Keep runtime override flags available across relevant subcommands.

## Non-Goals (Follow-Up Changes)

- `agentmux host relay --all` (multi-bundle hosting under one process)
- `agentmux host relay --watch` (bundle discovery/watch and dynamic reconcile)
- `agentmux stop <relay-id>` / `agentmux unhost <relay-id>`
- Interactive line-capture mode for `send` when stdin is a TTY and
  `--message` is omitted (cat-like input until EOF)

These follow-ups are tracked in notebook todos:
`agentmux:todos/runtime/8`, `agentmux:todos/runtime/9`,
`agentmux:todos/runtime/10`, and `agentmux:todos/runtime/11`.

## Impact

- Affected specs:
  - `cli-surface` (new)
- Affected code:
  - New `agentmux` CLI binary and command parser
  - Wrapper wiring in `agentmux-relay` and `agentmux-mcp`
  - Direct relay request path for CLI `list`/`send`
  - CLI integration tests and usage docs
