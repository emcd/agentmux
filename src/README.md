# Source Layout

This directory contains the runtime implementation for the `agentmux` binary.
The intended reader is a developer or coding agent changing contracts,
transport behavior, or CLI/MCP/TUI workflows.
End-user workflows are documented under `documentation/usage/`.

## Architecture Layers

- `bin/`
  - Entrypoints that invoke shared command execution.
  - See [src/bin/README.md](bin/README.md).
- `commands/`
  - CLI surface parsing, validation, and command dispatch (`host`, `up`, `down`,
    `list principals`, `look`, `send`, `tui`).
  - See [src/commands/README.md](commands/README.md).
- `runtime/`
  - Runtime-root resolution, bootstrap locks/socket binding, startup template
    hydration, inscriptions, and signal wiring.
  - See [src/runtime/README.md](runtime/README.md).
- `configuration/`
  - Bundle/coder/policy parsing and validation, plus session identity helpers.
  - Reads `tui/` to validate that a declared chord or behavior name exists,
    since the TUI owns those names. This is the one edge in the crate running
    from a foundational layer up into a surface layer; the layer order below it
    is otherwise the dependency order.
  - See [src/configuration/README.md](configuration/README.md).
- `protocol/`
  - The delivery protocol boundary: the mailbox, look, and submission-evidence
    vocabulary both delivery call directions name, so neither imports the other.
  - Imports nothing from `relay/`, `acp/`, `tmux/`, `pty/`, or `transports/`.
    `scripts/lint-delivery-protocol-boundary.py` fails a commit that gives it
    such an import, and checks that each of those directories still exists so
    the rule cannot pass vacuously after a rename. It does depend on
    `envelope.rs`.
- `relay/`
  - Relay IPC contracts, socket/client entrypoints, authorization checks,
    lifecycle actions, delivery engine, and stream registration/event routing.
  - See [src/relay/README.md](relay/README.md).
- `transports/`
  - The `Transport` trait, its dispatch enum, and the UI stream-broadcast
    transport. Concrete transports live in `acp/`, `tmux/`, and `pty/` (the
    last behind the `pty` Cargo feature).
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
- Starter configuration is scaffolded only into a configuration root resolved
  from the default tier, and never overwrites an existing file. A layer list the
  operator supplied by flag or environment is never scaffolded even when a layer
  is missing: answering "you named a layer that is not there" with a fresh empty
  deployment makes the mistake look like success.
- Delivery is asynchronous. There is no synchronous mode and no per-request mode
  selector: a `send` is accepted before its outcome is known. The relay
  guarantees an accepted message resolves at most once, not that it eventually
  resolves — see `delivery-quiescence`.
