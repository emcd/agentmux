# TUI Module

This module implements the interactive `agentmux tui` runtime.
It is developer-facing and describes code organization and state contracts.
User-facing usage details and keybindings are documented under
`documentation/usage/`.

## Surface Model

The TUI presents two co-equal top-level screen modes. Exactly one is active at
a time; `F4` toggles between them. The active mode is shown in the footer.

- **Communication** — owns send/receive: chat history, compose (`To` +
  `Message`), pending-delivery indicator, send dispatch. Default startup mode.
- **Interaction** — owns operator-driven session inspection: an interaction
  target header, the look snapshot, a raww dispatch input, and permission
  decisioning.

Help (`F1`), recipient picker (`F2`), and delivery events (`F3`) are overlays
available in both modes.

## Module Map

- `mod.rs`
  - top-level run loop and terminal lifecycle.
- `state/mod.rs`
  - app state model/types, the `ScreenMode` enum, and shared helpers for
    runtime error/status mapping.
- `state/history.rs`
  - chat history/event tracking, pending-delivery accounting, stream-event
    dedupe, and paging/snap behavior.
- `state/compose.rs`
  - compose/raww draft editing, mode-switch transitions, picker retargeting,
    and dispatch helpers.
- `state/relay.rs`
  - relay request/response plumbing, recipient refresh, and stream polling
    lifecycle.
- `input.rs`
  - mode-aware key handling and command intent updates.
- `render.rs`
  - per-mode pane rendering, overlays, and key help text.
- `target.rs`
  - recipient parsing/autocomplete and look-target resolution helpers.
- `workbench.rs`
  - launch option plumbing from CLI command layer.

## Behavior

- recipient discovery from relay `list` responses,
- two co-equal screen modes (Communication, Interaction) toggled with `F4`;
  per-mode cursor, draft, and scroll state preserved across switches,
- explicit `To` recipient field with deterministic target parsing,
- async send workflow with local pending tracking and terminal outcome updates,
- session identity precedence:
  - `--as-session`
  - `default-session` from active `tui.toml` configuration
  - no association fallback,
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
  - delivery + permission events,
- picker actions:
  - insert recipient into `To`,
  - set the interaction target and switch to Interaction mode (`l`),
  - set the interaction target, switch to Interaction mode, and focus the raww
    input (`w`),
- Interaction mode raww input dispatches raw writes to the active interaction
  target via relay `raww`,
- raww/permission region replacement: when the interaction target has pending
  permission requests and the raww input is empty, the permission decisioning
  pane occupies the raww region; otherwise the raww input occupies it,
- look snapshot rendering:
  - tmux targets: line snapshot rendering (`snapshot_lines`),
  - ACP targets: structured entry rendering by canonical kinds
    (`user`, `agent`, `cognition`, `invocation`, `result`, `update`),
- chat history viewport for sent/received messages,
- send workflow via relay `chat`,
- look workflow via relay `look`,
- raw write workflow via relay `raww`,
- stable rendering for validation/runtime error codes,
- stream reconnect handling with explicit `relay_unavailable` (not reachable)
  and `relay_timeout` (reachable but unresponsive/saturated) status,
- permission lifecycle handling:
  - pending queue visibility from relay stream events,
  - replay-safe dedupe keyed by `permission_request_id`,
  - session-scoped Interaction-mode resolution using ACP-native
    `permission.resolve` (`selected` with explicit `option_id` or `cancelled`),
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
