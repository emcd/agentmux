# TUI Compose / Interaction Helpers

This directory holds the per-mode interaction helpers split out
from the original monolithic `state.rs`. The split is by concern:
editing, pickers, interaction-mode, choice decisioning, and shared
text utilities.

The `AppState` type itself stays in `src/tui/state/mod.rs`; every
file in this directory is an `impl AppState` block that re-imports
the relevant types from the parent module via `super::{...}`.

## Directory layout

- `mod.rs`
  - Pure directory module: only `mod` declarations for the five sibling
    files, plus a `pub(super) use super::{...}` re-export of the
    types the sibling files need (`AppState`, `FocusField`,
    `LookSnapshotFormat`, `PendingChoiceEntry`, `PendingChoiceOption`,
    `PickerColumn`, `ScreenMode`, `ToCompletionState`,
    `append_recipient_token`, `current_recipient_token_context`,
    `map_relay_error`, `matching_recipient_candidates`). No state
    of its own.
- `editing.rs`
  - `impl AppState` for focus cycling, mode toggle, `To` /
    `Message` text and character editing, message cursor movement,
    `To` cursor + completion (`clear_to_completion`,
    `try_begin_to_completion`, `cycle_to_completion_candidate`,
    `accept_to_completion`), `clear_compose_fields`, and the
    Compose-mode cursor insertion helpers. The mode toggle also
    decides whether to auto-open the picker when entering
    Interaction mode without an active session.
- `pickers.rs`
  - `impl AppState` for the unified picker. Owns
    `open_picker` / `open_bundle_picker` / `close_picker`,
    `open_picker_focused`, the column-focus toggle, the
    column-scoped filter resolution (`column_filter`),
    visible-(filtered) index resolution, the session-insert and
    bundle-commit selection actions (`commit_selected_picker_session`
    resolves the mode-dependent meaning of committing a session row),
    the `toggle_events_overlay` / `toggle_help_overlay` overlay-open
    helpers (which enforce the mutual-exclusion invariant), and
    `dismiss_surfaces`, which clears the picker and both overlays so a
    surface-switching behavior lands on the mode beneath.
  - The help overlay's viewport offset lives here too
    (`scroll_help_overlay_*`, `set_help_overlay_viewport`). The
    overlay presents the whole binding surface, which is taller than a
    short terminal shows, so it is drawn through a viewport the
    operator moves. The bounds that offset is clamped against come
    from the renderer, since how many rows a column occupies is a
    function of wrapping at the width it was drawn at; this is the
    same direction `set_chat_history_viewport_height` runs in.
    `toggle_help_overlay` resets the offset on open, so the catalogue
    answers the same way however it was reached.
- `interaction.rs`
  - `impl AppState` for Interaction-mode entry
    (`enter_interaction_mode`, `enter_interaction_from_picker`),
    target-set helpers, raww draft editing plus cursor,
    snapshot scrolling (`scroll_interaction_snapshot_*`),
    the `navigate_interaction_*` helpers that move through the write
    draft when one is present and the snapshot when it is not,
    interaction-region visibility (`interaction_choice_active` is the
    predicate; `interaction_raww_region_visible` is its negation), the
    snapshot loader
    (`overlay_snapshot_from_payload`), and the
    `render_transport_label` helper consumed by the Interaction
    target header.
- `choices.rs`
  - `impl AppState` for Interaction-mode choice decisioning:
    `move_look_choice_request_selection`,
    `move_look_choice_option_selection`,
    `submit_choice_decision_selected`,
    `submit_choice_decision_cancelled`, the
    `selected_look_choice*` accessors, `look_pending_choices`,
    and `ensure_pending_choice_selection`. Choice selection
    resolution is single-sourced through
    `compare_pending_choice_order` so the snapshot and
    upsert ingestion paths cannot diverge.
- `text_util.rs`
  - Private `&str` cursor / line utilities shared across the
    compose submodules: `wrap_index`, `previous_char_boundary`,
    `next_char_boundary`, `line_ranges`, `line_range_for_cursor`,
    `line_and_column_for_index`, `cursor_index_for_line_column`.
    Used by `editing.rs` and `choices.rs` for cursor arithmetic
    that depends on character boundaries rather than byte
    indices.

## Cross-cutting invariants

- Every file is an `impl AppState` block; the `AppState` struct
  itself lives in `src/tui/state/mod.rs`. Cross-file state
  mutations happen through the same borrowed `&mut AppState`,
  so the helpers cannot pull in additional shared state.
- `compose/mod.rs` re-exports the typed surface from
  `src/tui/state/mod.rs` rather than reaching into
  `crate::tui::state::*` directly. This keeps the sibling impl
  blocks readable and avoids importing state internals that are
  not part of the compose domain.
- The picker column-focus invariant (one of `Bundles` /
  `Sessions` is focused at any time) is enforced inside
  `pickers.rs`; the other files do not flip `picker_focus`.
- Choice FIFO ordering is enforced by `compare_pending_choice_order`
  in `src/tui/state/history.rs` (a sibling, not mod.rs);
  `choices.rs` consumes that ordering rather than re-deriving it.
