# TUI Module

This module provides the interactive `agentmux tui` workbench.

## Current MVP Scope

- recipient discovery from relay `list` responses,
- explicit `To`/`Cc` recipient fields,
- `Tab` completion in recipient fields,
- recipient picker overlay (`F2`),
- send workflow via relay `chat` (`Ctrl+S`),
- look workflow via relay `look` (`Ctrl+L`),
- stable error-code rendering for validation failures.

## Keybindings

- `Esc` / `Ctrl+Q`: quit
- `Shift+Tab`: cycle field focus (`To` -> `Cc` -> `Message`)
- `Tab`: autocomplete recipient in active `To`/`Cc` field
- `F2`: open/close recipient picker overlay
- `Up` / `Down`: move recipient selection in picker overlay
- `Ctrl+S`: send message
- `Ctrl+L`: capture look snapshot
- `Ctrl+R`: refresh recipients
- `Ctrl+D`: toggle delivery mode (`async` / `sync`)
