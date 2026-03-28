# TUI Workbench Guide

`agentmux tui` is an interactive terminal workbench for recipient discovery,
messaging, and pane snapshots.

## Launch

```bash
agentmux tui
```

Optional sender override:

```bash
agentmux tui --sender master
```

## Keybindings

- `Ctrl+C`: quit
- `F1`: open/close help overlay
- `Tab`: focus next field (`To` <-> `Message`)
- `Shift+Tab`: cycle focus backward (`To` <-> `Message`)
- `Ctrl+Space`: trigger completion in `To`
- `Up` / `Down` in `To`: navigate active completion candidate
- `Up` / `Down` in `Message`: move cursor between message lines
- `Enter` in `To`: accept active completion and commit delimiter (`, `)
- `Enter` in `Message`: send message
- `Ctrl+J`: insert newline in `Message`
- `Esc` in `Message`: snap chat history viewport to latest
- `F2`: open/close recipient picker
- `F3`: open/close delivery events overlay
- `l` in picker: capture look snapshot for selected recipient
- `Esc` in look overlay: close look and return to picker context
- `PgUp` / `PgDn`: page chat history viewport backward/forward
- `Up` / `Down` in picker: move recipient selection
- `Ctrl+R`: refresh recipients
- mouse wheel: scroll chat history

## Status and Outcome Vocabulary

Connection state labels:

- `relay_unavailable`: relay socket not reachable
- `relay_timeout`: relay reachable but request timed out

Delivery outcomes:

- `accepted`: locally accepted and pending terminal completion
- `success`: terminal success
- `timeout`: terminal timeout
- `failed`: terminal failure with reason/reason_code when available

## Usage Notes

- Successful send clears `To` and `Message`.
- Recipient completion supports both `@`-triggered suggestions and manual
  trigger (`Ctrl+Space`).
- Terminal outcomes are sourced from relay completion updates keyed by
  `message_id`.
