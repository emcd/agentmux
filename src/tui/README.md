# TUI Module

This module provides the interactive `agentmux tui` workbench.

## Current MVP Scope

- recipient discovery from relay `list` responses,
- explicit `To` recipient field with deterministic target parsing,
- async-only send behavior with local pending-delivery tracking,
- recipient completion via `Ctrl+Space`,
- recipient picker overlay (`F2`),
- delivery events overlay (`F3`),
- send workflow via relay `chat` (`Ctrl+S`),
- look workflow via relay `look` (`Ctrl+L`),
- stable error-code rendering for validation failures.

## Keybindings

- `Esc` / `Ctrl+Q`: quit
- `Tab` / `Shift+Tab`: cycle field focus (`To` <-> `Message`)
- `Ctrl+Space`: autocomplete recipient in `To`
- `F2`: open/close recipient picker overlay
- `F3`: open/close events overlay
- `Up` / `Down`: move recipient selection in picker overlay
- `Ctrl+S`: send message
- `Ctrl+L`: capture look snapshot
- `Ctrl+R`: refresh recipients
