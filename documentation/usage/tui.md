# TUI Guide

`agentmux tui` is an interactive terminal surface for recipient discovery,
messaging, session inspection, and pane snapshots.

## Launch

```bash
agentmux tui
```

Optional session selector:

```bash
agentmux tui --as-session user@GLOBAL
```

Startup behavior:

- `agentmux tui` attempts relay auto-start when the resolved relay socket is
  unavailable.
- Auto-start uses the same resolved runtime roots as the active TUI launch
  (`--config-directory`, `--state-directory`, `--inscriptions-directory`).

## Screen Modes

The TUI has two co-equal top-level screen modes. Exactly one is active at a
time, and the footer shows which. `F4` toggles between them. Per-mode cursor,
draft, and scroll state is preserved across switches.

- **Communication** (default) — chat history and compose (`To` + `Message`)
  for send/receive workflows.
- **Interaction** — an interaction-target header, the look snapshot, a raww
  dispatch input, and permission decisioning for operator-driven session
  inspection.

When Interaction mode has no target, the header shows a placeholder hint:
open the picker (`F2`) and press `l` or `w` to choose a session.

## Keybindings

### Global

- `Ctrl+C`: quit
- `F1`: open/close help overlay
- `F2`: open/close recipient picker
- `F3`: open/close delivery events overlay
- `F4`: toggle between Communication and Interaction modes
- `Ctrl+R`: refresh recipients

### Communication mode

- `Tab` / `Shift+Tab`: cycle focus (`To` <-> `Message`)
- `Ctrl+Space`: trigger completion in `To`
- `Up` / `Down` in `To`: navigate active completion candidate
- `Up` / `Down` in `Message`: move cursor between message lines
- `Left` / `Right` / `Home` / `End` in `Message`: move cursor
- `Ctrl+A` / `Ctrl+E` in `Message`: move cursor to line start/end
- `Enter` in `To`: accept active completion and commit delimiter (`, `)
- `Enter` in `Message`: send message
- `Ctrl+J`: insert newline in `Message`
- `Esc` in `Message`: snap chat history viewport to latest
- `PgUp` / `PgDn`: page chat history viewport backward/forward
- mouse wheel: scroll chat history

### Interaction mode

- `PgUp` / `PgDn`: scroll look snapshot
- Raww input (active when raww has text, or no pending permission requests):
  - `Left` / `Right` / `Up` / `Down` / `Home` / `End`: move raww cursor
  - `Enter`: dispatch raww to the active interaction target via relay `raww`
  - `Ctrl+J`: insert newline in raww input
  - `Backspace`: delete the character before the raww cursor
- Permission decisioning (active when raww input is empty and the target has
  pending requests):
  - `Left` / `Right`: previous/next pending permission request for the target
  - `Up` / `Down`: previous/next ACP permission option
  - `Enter`: resolve selected request with selected option
    (`outcome=selected`)
  - `c`: resolve selected request as cancelled (`outcome=cancelled`)
- `Up` / `Down` with an empty raww input and no pending requests: scroll the
  look snapshot

### Recipient picker (`F2`)

- `Up` / `Down`: move recipient selection
- `Enter`: insert selected recipient into `To`
- `l`: set the interaction target and switch to Interaction mode
- `w`: set the interaction target, switch to Interaction mode, and focus the
  raww input
- `Esc` / `F2`: close picker

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
- Look snapshot rendering is transport-aware:
  - tmux look snapshots render line payloads directly.
  - ACP look snapshots render structured entries by kind:
    `user`, `agent`, `cognition`, `invocation`, `result`, `update`.
- Raww dispatch from Interaction mode routes through relay `raww` and surfaces
  acceptance-phase metadata when provided.
- Terminal outcomes are sourced from relay completion updates keyed by
  `message_id`.
- Permission requests are rendered from relay `permission.snapshot`,
  `permission.requested`, and `permission.resolved` events.
- Replay is at-least-once; the TUI deduplicates pending permission rows by
  `permission_request_id`.
- Permission decisions are ACP-native and explicit: selected option ids are
  forwarded verbatim via `permission.resolve`; cancelled decisions omit
  `option_id`.
