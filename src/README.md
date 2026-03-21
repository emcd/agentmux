# Source Layout

This directory contains the implementation for the unified `agentmux` binary.

## Module Map

- `bin/`
  - binary entrypoints.
  - See [src/bin/README.md](bin/README.md).
- `commands.rs`
  - CLI parsing and command execution for `host relay`, `host mcp`, `up`,
    `down`, `list`, `look`, and `send`.
- `configuration.rs`
  - bundle/coder TOML loading and sender resolution helpers.
- `envelope.rs`
  - relay envelope rendering + batching primitives.
- `relay.rs`
  - relay request/response contract and tmux delivery engine.
- `mcp/`
  - MCP stdio server surface and relay forwarding.
  - See [src/mcp/README.md](mcp/README.md).
- `tui/`
  - Interactive terminal workbench that composes `list`/`send`/`look` relay
    contracts for operator workflows.
- `runtime/`
  - path resolution, startup locks, association discovery, inscriptions, and
  starter template hydration.
  - See [src/runtime/README.md](runtime/README.md).
- `lib.rs`
  - crate module exports and shared startup helpers.

## Starter Template Embedding

Starter configuration templates are version-controlled and embedded via
`include_str!`:

- coders template: `data/configuration/coders.toml`
- bundle template: `data/configuration/bundle.toml`

Runtime startup writes these templates only when target files are missing.

## Delivery Notes

Relay `chat` currently supports `delivery_mode=async` and `delivery_mode=sync`.

Current async queue semantics:

- in-memory only (non-durable),
- FIFO ordering per target session,
- no dedupe/coalescing,
- no hard queue cap in MVP.

Pending async entries are lost if relay exits or restarts.
