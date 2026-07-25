# TUI Render

This directory holds the per-frame drawing code for the
`agentmux tui` workbench. It is the implementation of the render
contracts described in the parent `src/tui/README.md`.

The split is structural, not behavioral: every entry point is invoked
from the top-level `render` function in `frame.rs`, and the per-mode
and per-overlay files are sibling helpers reached via
`render::render(frame, state)` in `src/tui/mod.rs`. State ownership
stays in `src/tui/state/`; the render layer borrows `&mut AppState`
purely for reads, never for writes.

## Directory layout

- `mod.rs`
  - Pure directory module: only `mod` declarations and `pub(crate) use
    frame::render` so the run loop can call `render::render(frame,
    &mut state)` without reaching into `frame` directly.
- `frame.rs`
  - Top-level `render` entry, `render_header` / `render_main` /
    `render_footer`, and the frame-level layout constants
    (`WORKBENCH_MIN_CHAT_HEIGHT`, `WORKBENCH_MIN_COMPOSE_HEIGHT`,
    `INTERACTION_RAWW_PANE_HEIGHT`, `INTERACTION_TARGET_HEADER_HEIGHT`).
  - Composes the main pane via vertical layout, then dispatches to
    `render_communication_mode` or `render_interaction_mode`. The
    active cursor is suppressed whenever any overlay flag
    (`help_overlay_open`, `picker_open`, `events_overlay_open`) is
    set, so the terminal cursor never appears while a modal is open.
- `communication.rs`
  - `render_communication_mode`, the chat history pane, the compose
    pane (`To` + `Message`), and the per-session `Recipient` rendering
    shared with the picker.
- `interaction.rs`
  - `render_interaction_mode`, the target header, the look snapshot
    pane (tmux line-mode and ACP structured-entry-mode rendering),
    the raww pane, and the choice decisioning pane (which replaces
    the raww pane when an active look target has pending choices).
- `cursor.rs`
  - Active cursor placement: compose cursor in Communication mode,
    raww cursor in Interaction mode. Position helpers that depend on
    the geometry module's `compose_message_layout` /
    `compose_message_visible_start` / `compose_titled_block` /
    `raww_titled_block` / `split_workbench_rows`.
- `geometry.rs`
  - Shared measure/layout helpers. `centered_rect`,
    `split_workbench_rows`, `compute_compose_height`, titled-block
    builders (`workbench_titled_block`, `compose_titled_block`,
    `raww_titled_block`), `MessageLayout`,
    `compose_message_layout`, `compose_message_visible_start`, and
    `wrap_text`.
- `overlays/`
  - Modal overlays drawn on top of the main pane. See
    `overlays/README.md` for the per-file shape and the
    help/picker/events responsibilities.

## Render order

The run loop (`src/tui/mod.rs`) calls `render::render(frame, &mut
state)` whenever its `needs_redraw` flag is set. `needs_redraw` is
initialized to `true` at loop start (`mod.rs:51`), so the first
frame renders before any input or relay poll; it is also set to
`true` after every input event handled (`mod.rs:72`) and when
`poll_relay_events` reports new stream events (`mod.rs:85`). The
order of a typical iteration is:

1. Clear the frame (ratatui default).
2. `render_header` / `render_main` / `render_footer` draw the
   per-mode main pane.
3. `render_active_cursor` positions the terminal cursor in the
   active editor field; it is a no-op when any overlay is open.
4. Overlays are drawn in `help` → `picker` → `events` order, gated
   by their open flags.

The single-entry-point shape lets the run loop stay free of any
ratatui types and limits the structural surface area to
`render::render(frame, &mut state)`. Both overlays and the
main-pane render read `AppState` directly; no render-only state
lives in this directory.

## Cross-cutting invariants

- The render layer is read-only with respect to `AppState`; all
  state mutations happen in `src/tui/state/` in response to input
  or relay events.
- Layout constants live in `frame.rs` so the floor / minimum
  pane sizes stay in a single place; `geometry.rs` consumes them
  rather than re-declaring them.
- Cursor placement is conditional on overlay state so the operator
  never sees a stale cursor while a modal is open.
