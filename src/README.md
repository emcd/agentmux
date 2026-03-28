# Source Layout

This directory contains the runtime implementation for the `agentmux` binary.
The intended reader is a developer or coding agent changing contracts,
transport behavior, or CLI/MCP/TUI workflows.

## Architecture Layers

- `bin/`
  - Entrypoints that invoke shared command execution.
  - See [src/bin/README.md](bin/README.md).
- `commands/`
  - CLI surface parsing, validation, and command dispatch (`host`, `up`, `down`,
    `list`, `look`, `send`, `tui`).
  - See [src/commands/README.md](commands/README.md).
- `runtime/`
  - Runtime-root resolution, bootstrap locks/socket binding, startup template
    hydration, inscriptions, and signal wiring.
  - See [src/runtime/README.md](runtime/README.md).
- `configuration.rs`
  - Bundle/coder/policy parsing and validation, plus session identity helpers.
- `relay.rs` + `relay/`
  - Relay IPC contracts, authorization checks, lifecycle actions, delivery
    engine, and stream registration/event routing.
  - See [src/relay/README.md](relay/README.md).
- `mcp/`
  - MCP server handlers that validate MCP payloads and forward relay requests.
  - See [src/mcp/README.md](mcp/README.md).
- `tui/`
  - Interactive workbench state/input/render loop on top of relay contracts.
  - See [src/tui/README.md](tui/README.md).
- `envelope.rs`
  - Envelope rendering and batching primitives used by delivery paths.
- `lib.rs`
  - Crate exports and shared startup banner helper.

## Cross-Cutting Invariants

- Relay is the authorization decision point; CLI/MCP/TUI perform request-shape
  validation and pass relay denial details through.
- Runtime starter files are hydrated only when absent from config root.
- Delivery supports `async` and `sync`; ACP sync acknowledges at dispatch/first
  activity boundaries and correlates completion by `message_id`.
