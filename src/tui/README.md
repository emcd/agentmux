# TUI Module

This module implements the interactive `agentmux tui` workbench runtime.
It is developer-facing and describes code organization and state contracts.
User-facing usage details and keybindings are documented under
`documentation/usage/`.

## Module Map

- `mod.rs`
  - top-level run loop and terminal lifecycle.
- `state/mod.rs`
  - app state model/types and shared helpers for runtime error/status mapping.
- `state/history.rs`
  - chat history/event tracking, pending-delivery accounting, stream-event
    dedupe, and paging/snap behavior.
- `state/relay.rs`
  - relay request/response plumbing, recipient refresh, and stream polling
    lifecycle.
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
- session identity precedence:
  - `--as-session`
  - `default-session` from active `tui.toml` configuration
  - no association fallback in MVP,
- bundle precedence:
  - `--bundle`
  - `default-bundle` from active `tui.toml` configuration,
- delivery outcome vocabulary:
  - `accepted`, `success`, `timeout`, `failed`,
- recipient completion via `@` token triggers plus explicit manual trigger,
- `@`-prefixed tokens trigger immediate completion proposals after one suffix character,
- overlays:
  - help,
  - recipient picker,
  - delivery events,
  - look snapshot,
- look snapshot rendering:
  - tmux targets: line snapshot rendering (`snapshot_lines`),
  - ACP targets: structured entry rendering by canonical kinds
    (`user`, `agent`, `cognition`, `invocation`, `result`, `update`),
- chat history viewport for sent/received messages,
- send workflow via relay `chat`,
- look workflow via relay `look`,
- stable rendering for validation/runtime error codes,
- stream reconnect handling with explicit `relay_unavailable` (not reachable)
  and `relay_timeout` (reachable but unresponsive/saturated) status,
- startup relay auto-spawn fallback when relay socket is unavailable, using the
  same resolved configuration/state/inscriptions roots as the active TUI
  launch.

## Stream and State Notes

- TUI connects as relay stream client class `ui`.
- Stream event dedupe is keyed by stable identifiers in app state to avoid
  duplicate status/event lines after reconnect.
- `accepted` is process-local; terminal outcomes come from relay completion
  results/events.

## User Docs

- Usage guide: `documentation/usage/tui.md`
