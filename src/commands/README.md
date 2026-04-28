# Commands Module

This directory owns the unified CLI surface for `agentmux`.

## Responsibilities

- Parse command-line arguments into typed command structs.
- Validate command inputs before runtime/relay calls.
- Resolve runtime root overrides for every command family.
- Render human and machine output for command responses.

## File Map

- `mod.rs`
  - top-level command router and shared command argument/result structs.
- `host.rs`
  - `agentmux host relay` and `agentmux host mcp`.
  - relay listener/process loop, lifecycle startup summary emission,
    no-autostart mode, and connection-worker orchestration.
- `lifecycle.rs`
  - shared `up`/`down` transition execution helpers.
- `up.rs`
  - `agentmux up` selector parsing and execution.
- `down.rs`
  - `agentmux down` selector parsing and execution.
- `list.rs`
  - `agentmux list sessions`.
- `look.rs`
  - `agentmux look`.
- `raww.rs`
  - `agentmux raww` direct-write request surface.
- `send.rs`
  - `agentmux send`, including stdin/message precedence and timeout fields.
- `tui.rs`
  - `agentmux tui` launch path, session/default precedence wiring, and relay
    auto-spawn fallback using resolved runtime roots.
- `shared.rs`
  - reusable parsing/output helpers shared across command handlers.

## Operational Notes

- Bare `agentmux` dispatches to TUI only in interactive TTY mode.
- `host relay --no-autostart` is process-only mode and must not report
  autostart failures for bundles.
- Worker-pool overload and pre-hello idle handling are implemented in
  `host.rs` and covered by integration tests under `tests/integration/cli/`
  and `tests/integration/relay_delivery_runtime.rs`.
