# TUI Module

This module provides the interactive `agentmux tui` workbench.

## Current MVP Scope

- recipient discovery from relay `list` responses,
- explicit `To` recipient field with deterministic target parsing,
- async-only send behavior with local pending-delivery tracking,
- sender identity precedence:
  - `--sender`
  - `.auxiliary/configuration/agentmux/overrides/tui.toml` (debug/testing)
  - `<config-root>/tui.toml`
  - association fallback,
- delivery outcome vocabulary mapping:
  - `accepted`, `success`, `timeout`, `failed`,
- recipient completion via context-sensitive `Tab` plus `Ctrl+Space`,
- `@`-prefixed tokens trigger immediate completion proposals after one suffix character,
- recipient picker overlay (`F2`),
- delivery events overlay (`F3`),
- look snapshot overlay (`F4`),
- help overlay (`F1`),
- chat history viewport with PgUp/PgDn navigation for sent/received messages,
- send workflow via relay `chat` (`Ctrl+S`),
- look workflow via relay `look` (`F4`),
- stable error-code rendering for validation failures.
- stream reconnect handling with explicit `relay_unavailable` status when
  disconnected.

## Keybindings

- `Esc` / `Ctrl+Q`: quit
- `F1`: open/close help overlay
- `Tab`: in `To`, cycle/start completion when applicable; otherwise move focus
- `Shift+Tab`: cycle field focus backward (`To` <-> `Message`)
- `Ctrl+Space`: cycle/start completion in `To`
- `Enter`: accept active completion proposal in `To`
- `F2`: open/close recipient picker overlay
- `F3`: open/close events overlay
- `F4`: capture look snapshot for selected recipient (or first `To` recipient) and open overlay
- `PgUp` / `PgDn`: page chat history viewport backward/forward
- `Up` / `Down`: move recipient selection in picker overlay
- `Ctrl+S`: send message
- `Ctrl+R`: refresh recipients
