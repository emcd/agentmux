# TUI State

This directory holds the `AppState` model and every per-frame or
per-event state transition for the `agentmux tui` workbench. It is
the implementation of the state contracts described in the parent
`src/tui/README.md`.

The split is by concern: the `AppState` struct and the cross-cutting
relay-error mapping live in `mod.rs`; the chat / pending-delivery
machines live in `history.rs`; the relay request/response plumbing
lives in `relay.rs`; and the per-mode interaction helpers live in
`compose/`.

## Directory layout

- `mod.rs`
  - `AppState` (the central state struct), `TuiLaunchOptions` (the
    startup configuration consumed by `AppState::new`), the
    `ScreenMode` / `FocusField` / `PickerColumn` enums, the
    `ChatHistoryDirection` / `ChatHistoryEntry` / `StatusEntry` /
    `Recipient` / `PendingChoiceEntry` / `PendingChoiceOption` /
    `LookSnapshotFormat` types, and the cross-cutting
    `map_relay_error` / `map_relay_request_failure` /
    `is_relay_timeout_error` / `is_relay_unavailable_error` helpers
    that surface canonical relay error codes (`validation_*`,
    `relay_unavailable`, `authorization_forbidden`) to the render
    layer.
  - The state-history depth constants (`STATUS_HISTORY_MAXIMUM`,
    `EVENT_HISTORY_MAXIMUM`, `CHAT_HISTORY_MAXIMUM`,
    `SEEN_STREAM_IDS_MAXIMUM`) live here.
  - Tests live under `tests/unit/` in `tui.rs`, `tui_bindings.rs`,
    `tui_dispatch.rs`, `tui_help.rs`, `tui_workbench/`,
    `tui_session.rs`, and
    `tui_relay_error_mapping.rs`. `src/tui/` carries one inline
    `#[cfg(test)]`, in `render/overlays/help.rs`, covering a
    crate-private renderer no public interface reaches.
- `history.rs`
  - `impl AppState` for the chat-history and stream-event domain:
    `send_message`, `record_chat_events`, `record_stream_events`,
    chat-history paging (`scroll_chat_history_*`, `snap_*`,
    `set_chat_history_viewport_height`, `set_chat_history_total_lines`),
    pending-delivery accounting, and the seen-ids sets that dedupe
    incoming messages and delivery outcomes across reconnects.
- `relay.rs`
  - `impl AppState` for relay request plumbing and stream
    lifecycle: `refresh_recipients`, `refresh_cross_bundle_candidates`,
    `request_relay`, `poll_relay_events`, the stream-poll error
    retry gate, and the recipient-list application that drives
    picker selection.
- `compose/`
  - The per-mode interaction helpers, split by concern. See
    `compose/README.md` for the per-file shape and the editing /
    picker / interaction / choice responsibilities.

## State flow

The TUI follows a single-direction state flow:

1. `src/tui/mod.rs` constructs `AppState` from `TuiLaunchOptions`
   and calls `state.refresh_recipients()` once at startup.
2. The run loop reads terminal events, dispatches to `input.rs`,
   and polls relay stream events. Each consumer mutates the
   `AppState` in place.
3. After every input or relay-poll mutation, the run loop calls
   `render::render(frame, &mut state)` to redraw. The render layer
   borrows `AppState` immutably or mutably (cursor placement) and
   never mutates state.

`AppState` is the single shared mutable object across the entire
TUI runtime.

## Cross-cutting invariants

- Relay errors whose canonical code is `validation_*`,
  `relay_unavailable`, or `authorization_forbidden` are surfaced to
  the render layer as `RuntimeError::Validation` with the code
  intact (see `map_relay_error`). Without the explicit
  `authorization_forbidden` arm a relay-enforced permission denial
  would collapse into a generic IO status with no code, even though
  the TUI surface requires the code to be visible. Every other
  (internal) code is classified as an IO status rather than an
  actionable validation code, but the code survives in the
  diagnostic message (`io::Error::other(format!("relay error {}",
  error.code))`) so the real cause is not collapsed into one opaque
  string.
- Stream-event dedupe is keyed by stable identifiers on `AppState`
  (incoming `message_id`, `delivery_outcome` id, `choice_request_id`)
  so duplicate history lines and choice rows do not appear after
  reconnect.
- `accepted` is process-local; terminal outcomes come from relay
  completion results/events, not from the `Send` response.
- The TUI identifies itself to the relay by `principal_id`
  (a `<session>@<namespace>` string); the relay does not gate
  stream clients on a class field.
