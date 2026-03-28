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
- recipient completion via `@` token triggers plus `Ctrl+Space`,
- `@`-prefixed tokens trigger immediate completion proposals after one suffix character,
- overlays:
  - help (`F1`),
  - recipient picker (`F2`),
  - delivery events (`F3`),
  - look snapshot (opened from picker context action),
- chat history viewport with PgUp/PgDn navigation for sent/received messages,
- send workflow via relay `chat` (`Enter` in `Message`),
- look workflow via relay `look` (`l` in recipient picker),
- stable rendering for validation/runtime error codes,
- stream reconnect handling with explicit `relay_unavailable` (not reachable)
  and `relay_timeout` (reachable but unresponsive/saturated) status.

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
- `l` in picker overlay: capture look snapshot for selected recipient and open overlay
- `Esc` in look overlay: close look and return to picker context
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
