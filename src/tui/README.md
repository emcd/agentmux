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
  target header, the look snapshot, a Write input (relay `raww`), and
  permission decisioning.

Help (`F1`), recipient picker (`F2`), delivery events (`F3`), and bundle
picker (`F5`) are overlays available in both modes.

## Module Map

- `mod.rs`
  - top-level run loop and terminal lifecycle.
- `state/mod.rs`
  - app state model/types, the `ScreenMode` enum, and shared helpers for
    runtime error/status mapping.
- `state/history.rs`
  - chat history/event tracking, pending-delivery accounting, stream-event
    dedupe, and paging/snap behavior.
- `state/compose/`
  - compose/raww draft editing, mode-switch transitions, picker retargeting,
    and dispatch helpers. Split by concern:
    - `mod.rs` — pure hub: submodule decls and re-exports of `AppState` and
      sibling-state items
    - `pickers.rs` — recipient/bundle picker open/close/move, picker
      selection insert, overlay toggles (`toggle_events_overlay`,
      `toggle_help_overlay`)
    - `editing.rs` — focus cycling, mode toggle, text/character editing,
      message cursor, `To` completion, `clear_compose_fields`
    - `interaction.rs` — interaction-mode entry, target set, raww draft
      editing + cursor, snapshot scrolling, interaction-region visibility,
      `overlay_snapshot_from_payload`, `render_transport_label`
    - `permissions.rs` — look-permission selection/resolve,
      `ensure_pending_permission_selection`, `selected_look_permission*`,
      `look_pending_permissions`, `submit_permission_decision`
    - `text_util.rs` — private `&str` cursor/line utilities shared across
      the compose submodules
- `state/relay.rs`
  - relay request/response plumbing, recipient refresh, and stream polling
    lifecycle.
- `input.rs`
  - mode-aware key handling and command intent updates.
- `render/`
  - per-mode pane rendering, overlays, and key help text. Split by area:
    - `mod.rs` — pure hub: submodule decls and `pub(crate) use frame::render`
    - `frame.rs` — top-level `render` entry, `render_header`/`render_main`/
      `render_footer`, frame-level layout constants
    - `communication.rs` — `render_communication_mode`, compose + chat
      history panes, peer resolution
    - `interaction.rs` — `render_interaction_mode`, target header, raww
      pane, look snapshot, ACP entries, look permission lines
    - `overlays/`
      - `mod.rs` — pure hub: submodule decls
      - `recipient_picker.rs` — recipient picker overlay, hint strip,
        per-row readiness styling
      - `bundle_picker.rs` — bundle picker overlay, status header line,
        severity styling
      - `events.rs` — events overlay (pending permissions + delivery
        events)
      - `help.rs` — help overlay (two-column keybinding reference)
    - `cursor.rs` — active cursor, compose/raww cursor placement, position
      + column helpers, raww pane area
    - `geometry.rs` — shared measure/layout helpers (`centered_rect`,
      `split_workbench_rows`, `compute_compose_height`, titled blocks,
      `MessageLayout`, `compose_message_layout`,
      `compose_message_visible_start`, `wrap_text`, `raww_titled_block`)
- `target.rs`
  - recipient parsing/autocomplete and look-target resolution helpers.
- `workbench.rs`
  - launch option plumbing from CLI command layer.

## Behavior

- recipient discovery from relay `list` responses,
- two co-equal screen modes (Communication, Interaction) toggled with `F4`;
  per-mode cursor, draft, and scroll state preserved across switches,
- explicit `To` recipient field with deterministic target parsing. The relay
  requires fully-qualified targets, so the client fills in the namespace before
  dispatch and always emits `session@bundle`:
  - bare `session` is qualified with the sender's bound bundle and emitted as
    `session@bound-bundle`,
  - canonical `session@bundle` is emitted verbatim, routing to the named bundle
    (peer bundle or the relay-wide `@GLOBAL` user registry),
  - a relay-wide sender (`@GLOBAL` principal, no bound bundle) has no bundle to
    qualify a bare target with, so a bare target is rejected at compose time with
    `validation_unqualified_target`; it must name each target's `@namespace`
    explicitly, because the relay derives `Send` routing for a relay-wide
    principal solely from target suffixes,
  - `parse_tui_target_identifier` rejects empty halves, `/`, and more than one
    `@` separator at compose time,
- async send workflow with local pending tracking and terminal outcome updates,
- session identity precedence:
  - `--as-session`
  - `default-session` from active `tui.toml` configuration
  - no association fallback,
- bundle precedence (interactive launch does not require a bundle; a fresh
  install ships none, and the operator picks one in the picker):
  - `--bundle`
  - `default-bundle` from active `users.toml` configuration
  - the first available bundle, else an empty browsing context,
- delivery outcome vocabulary:
  - `accepted`, `success`, `timeout`, `failed`,
- recipient completion via `@` token triggers plus explicit manual trigger,
- `@`-prefixed tokens trigger immediate completion proposals after one suffix character,
- overlays:
  - help,
  - recipient picker: lists routable recipients with a one-line hint strip at
    the foot showing the active keybindings. The `Enter` hint is
    context-sensitive — it reads `Insert into To` in Communication mode and
    `Open (look+raww)` in Interaction mode — alongside `Esc` Close and
    `Up/Down` Move.
  - delivery + permission events,
  - bundle picker (F5): active bundle switching — browses
    `available_bundles` (sourced from `load_bundle_group_memberships` at TUI
    launch), highlights the active bundle, and on Enter replaces the active
    bundle context. The switch rebuilds the bundle-bound `RelayStreamSession`,
    resets bundle-scoped state (recipients, `last_selected_recipient`, bundle
    status, look snapshot, pending permissions, chat history, delivery
    bookkeeping, write draft), and triggers `refresh_recipients` on the new
    bundle. Selecting the active bundle is a no-op that closes the picker.
    Cross-bundle targeting for `Send` and `Look` is handled separately via the
    `session@bundle` grammar in the `To` / look-target field: the relay resolves
    the peer bundle by suffix and authorizes the requester's capability at the
    uniform cross-bundle `all:all` scope (the same threshold for `send` and
    `look`; unknown peers/targets surface as `validation_unknown_bundle` /
    `validation_unknown_target`). `Raww` routes the same way: the relay derives
    the peer bundle from the look-target's `session@bundle` suffix and authorizes
    `raww` at the same uniform `all:all` cross-bundle scope (issues/relay/24).
    Because `Send` and `Raww` routing is suffix-based, the shared relay client
    omits the wire-envelope `namespace` on those frames for every caller (TUI and
    MCP alike); the browsing bundle survives only as the `List` / recipient-picker
    enumeration context and the `Look` namespace selector, never as a sender
    binding (so a relay-wide `@GLOBAL` sender shows no `Bundle:` field in the
    header),
- picker actions (mode-aware `Enter`, no separate `l` / `w` keys):
  - Communication mode: insert the selected recipient into `To`,
  - Interaction mode: open the Interaction screen for the selected identity,
    running a synchronous relay `Look` so the look pane is populated with
    recent session history before the Write input takes focus,
- picker last-selected persistence: the most recently committed picker target
  (Communication insert or Interaction open) is tracked by session name in
  `last_selected_recipient`; when the picker reopens or the recipient list
  refreshes, selection is resolved by name against the current list and falls
  back deterministically to index 0 when the prior target is absent,
- picker bundle status header: one-line CLI-style key=value summary of
  `bundle.hosted`, `bundle.state`, `bundle.startup_health`, and reason code
  from `relay::ListedBundle`, color-coded into four severity buckets
  (`Healthy`/`Degraded`/`HostedDown`/`Unhosted`) so `hosted=true, state=down`
  (sessions failed to start) is visually distinct from `hosted=false,
  state=down` (not hosted),
- picker per-session readiness: rows sourced from `relay::ListedSession`
  surface `ready` directly — ready rows render in default style, not-ready
  rows render dimmed (`DarkGray`/`DIM`) and gain a trailing `[not ready]`
  marker so the state survives color stripping,
- Interaction mode Write input dispatches raw writes to the active interaction
  target via relay `raww`,
- Write/permission region replacement: when the interaction target has pending
  permission requests and the Write input is empty, the permission
  decisioning pane occupies the region; otherwise the Write input occupies it,
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
