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
- **Interaction** — an interaction-target header, the look snapshot, a Write
  input, and choice decisioning for operator-driven session inspection.

When Interaction mode has no target, the header shows a placeholder hint:
open the picker (`F2`) and press `Enter` to choose a session.

## Keybindings

### Global

- `Ctrl+C`: quit
- `F1`: open/close help overlay
- `F2`: open/close recipient picker
- `F3`: open/close delivery events overlay
- `F4`: toggle between Communication and Interaction modes
- `F5`: open/close bundle picker
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
- Write input (active when write has text, or no pending choice requests):
  - `Left` / `Right` / `Up` / `Down` / `Home` / `End`: move write cursor
  - `Enter`: dispatch write to the active interaction target via relay `raww`
  - `Ctrl+J`: insert newline in write input
  - `Backspace`: delete the character before the write cursor
- Choice decisioning (active when write input is empty and the target has
  pending requests):
  - `Left` / `Right`: previous/next pending choice request for the target
  - `Up` / `Down`: previous/next ACP choice option
  - `Enter`: resolve selected request with selected option
    (`outcome=selected`)
  - `c`: resolve selected request as cancelled (`outcome=cancelled`)
- `Up` / `Down` with an empty write input and no pending requests: scroll the
  look snapshot

### Recipient picker (`F2`)

The picker header surfaces a one-line bundle status in CLI-style key=value
format (`bundle=NAME hosted=yes|no state=up|down ...`) and color-codes it:

- green: hosted and healthy,
- yellow: hosted and degraded,
- red (bold): hosted but `state=down` (sessions failed to start),
- gray: not hosted (never started or shut down).

This distinguishes `hosted=true, state=down` (bundle is up but every session
failed startup) from `hosted=false, state=down` (bundle has not been started).

Each recipient row reflects the session's per-session readiness from the relay
list payload. Sessions that are not yet ready render dimmed and gain a
trailing `[not ready]` marker so the state is legible even without color.

- `Up` / `Down`: move recipient selection
- `Enter` (Communication mode): insert selected recipient into `To`
- `Enter` (Interaction mode): open the Interaction screen for the selected
  identity — the relay `Look` runs synchronously so the look pane is populated
  with recent session history before the Write input takes focus
- `Esc` / `F2`: close picker

The picker remembers the most recently committed recipient by session name
across close/reopen and across recipient list refreshes. When the prior
target is no longer present in the current list, the selection falls back
deterministically to the first available session.

### Bundle picker (`F5`)

The bundle picker browses the bundles discovered at TUI launch (from configured
bundle files) and lets the operator switch which bundle the TUI is targeting.

- `Up` / `Down`: move bundle selection
- `Enter`: switch the active bundle context
- `Esc` / `F5`: close picker

The active bundle is highlighted and labeled `[active]`. Selecting it again
closes the picker as a no-op. Selecting a different bundle:

- replaces the active bundle context (header `Bundle:` indicator reflects the
  new bundle),
- rebuilds the relay stream session with the new bundle,
- clears bundle-scoped state (recipients, last-selected recipient, bundle
  status, look snapshot, pending choices, chat history, delivery state,
  write draft),
- triggers a recipient refresh against the new bundle; if the new bundle is
  unhosted/unreachable, the refresh fails fast and surfaces a relay error in
  the status pane (the bundle context stays switched).

Cross-bundle targeting is governed by policy scope: reaching sessions in
another bundle requires the `all` scope for the operation, and the relay
denies insufficient scope with `authorization_forbidden`.

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
- Write dispatch from Interaction mode routes through relay `raww`; delivery is
  asynchronous. The write is acknowledged immediately as `queued`, and its
  terminal outcome arrives later as a `delivery_outcome` stream event.
- Terminal outcomes are sourced from relay completion updates keyed by
  `message_id`, and `raww` writes are tracked and deduped the same way as chat
  sends.
- Choice requests are rendered from relay `choices.snapshot`,
  `choices.requested`, and `choices.resolved` events.
- Replay is at-least-once; the TUI deduplicates pending choice rows by
  `choice_request_id`.
- Choice decisions are ACP-native and explicit: selected option ids are
  forwarded verbatim via `choices.pick`; cancelled decisions omit
  `option_id`.
