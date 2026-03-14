# Change: Add TUI transport prerequisites for history viewport work

## Why

Before implementing TUI chat history (`todos/tui/4`), we need explicit
contracts for sender identity binding and relay-to-TUI delivery/update flow.
Without that lock, history behavior can drift across CLI, relay, and TUI
surfaces.

## What Changes

- Define TUI sender identity precedence for startup and runtime:
  - CLI `--sender`
  - `tui.toml` default sender
  - runtime association fallback
  - fail-fast when unresolved
- Define a relay-level structured event flow for TUI consumption so inbound
  messages and delivery outcomes share a canonical payload schema.
- Lock ack/outcome mapping used by TUI state/history:
  - `accepted`, `success`, `timeout`, `failed`
- Define reconnect/error behavior as explicit fail-fast contracts.
- Keep same-bundle-only scope lock for MVP transport/history behavior.

## Impact

- Affected specs:
  - `tui-surface`
  - `session-relay`
  - `runtime-bootstrap`
- Affected code (implementation follow-up, not in this proposal):
  - `src/tui/*`
  - relay request/response/event plumbing
  - runtime config/association resolution for TUI startup
