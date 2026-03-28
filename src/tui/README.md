# TUI Module

This module provides the interactive `agentmux tui` workbench.

## Module Map

- `mod.rs`
  - top-level run loop and terminal lifecycle.
- `state.rs`
  - app state model, relay stream client, recipient cache, event/chat history,
    pending delivery tracking, and error/status surfaces.
- `input.rs`
  - key handling and command intent updates.
- `render.rs`
  - pane rendering, overlays, and key help text.
- `target.rs`
  - recipient parsing/autocomplete and look-target resolution helpers.
- `workbench.rs`
  - launch option plumbing from CLI command layer.

## Current MVP Behavior

- recipient discovery from relay `list` responses,
- explicit `To` recipient field with deterministic target parsing,
- async send workflow with local pending tracking and terminal outcome updates,
- sender identity precedence:
  - `--sender`
  - `.auxiliary/configuration/agentmux/overrides/tui.toml` (debug/testing)
  - `<config-root>/tui.toml`
  - association fallback,
- delivery outcome vocabulary:
  - `accepted`, `success`, `timeout`, `failed`,
- recipient completion:
  - `@` token triggers immediate proposals after one suffix character,
  - manual trigger via `Ctrl+Space`,
- overlays:
  - help (`F1`), recipient picker (`F2`), events (`F3`), look (`F4`),
- chat history viewport with PgUp/PgDn paging,
- stable rendering for validation/runtime error codes,
- stream reconnect handling with `relay_unavailable` status when disconnected.

## Keybindings

- `Ctrl+C`: quit
- `F1`: open/close help overlay
- `Tab`: focus next field (`To` <-> `Message`)
- `Shift+Tab`: cycle field focus backward (`To` <-> `Message`)
- `Ctrl+Space`: trigger completion in `To`
- `Up` / `Down` in `To`: navigate active completion candidate
- `Up` / `Down` in `Message`: move cursor between message lines
- `Enter`:
  - in `To`, accept active completion proposal
  - in `Message`, send message
- `Ctrl+J`: insert newline in `Message`
- `Esc` in `Message`: snap chat history viewport to latest
- `F2`: open/close recipient picker overlay
- `F3`: open/close events overlay
- `F4`: capture look snapshot for selected recipient (or first `To` recipient) and open overlay
- `PgUp` / `PgDn`: page chat history viewport backward/forward
- `Up` / `Down`: move recipient selection in picker overlay
- `Ctrl+R`: refresh recipients
- mouse wheel: scroll chat history
- successful send clears `To` and `Message` fields

## Stream and State Notes

- TUI connects as relay stream client class `ui`.
- Stream event dedupe is keyed by stable identifiers in app state to avoid
  duplicate status/event lines after reconnect.
- `accepted` is process-local; terminal outcomes come from relay completion
  results/events.
