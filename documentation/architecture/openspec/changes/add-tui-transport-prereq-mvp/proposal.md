# Change: Add TUI transport prerequisites for history viewport work

## Why

Before implementing TUI chat history (`todos/tui/4`), we need explicit
TUI-facing contracts for sender identity resolution and history/delivery state
presentation. Transport-stream mechanics are tracked in a dedicated adjacent
change to keep this proposal focused.

## What Changes

- Define TUI sender identity precedence for startup/runtime:
  - CLI `--sender`
  - local testing override sender file (debug/testing only)
  - normal `<config-root>/tui.toml` sender
  - runtime association fallback
  - fail-fast when unresolved
- Define TUI delivery-state mapping used by history/status views:
  - `accepted`, `success`, `timeout`, `failed`
- Define reconnect/error semantics for TUI state handling.
- Keep same-bundle-only scope lock for MVP transport/history behavior.
- Lock bare `agentmux` startup dispatch:
  - interactive TTY with no subcommand starts TUI
  - non-TTY with no subcommand prints help and exits non-zero
- Depend on adjacent transport contract change
  `add-relay-stream-hello-transport-mvp` for long-lived relay stream details.

## Impact

- Affected specs:
  - `cli-surface`
  - `tui-surface`
  - `runtime-bootstrap`
- Affected code (implementation follow-up, not in this proposal):
  - `src/tui/*`
  - runtime config/association resolution for TUI startup
  - TUI state mapping from relay push events (defined by adjacent relay
    transport change)
