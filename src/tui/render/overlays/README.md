# TUI Render Overlays

This directory holds the modal overlays drawn on top of the main
pane by `src/tui/render/frame.rs`. Each overlay is a single
`render_<name>_overlay` function invoked once per redraw when its
open flag is set on `AppState`.

The overlay surfaces are governed by `openspec/specs/tui-surface/spec.md`
(Communication/Interaction mode interactions, picker behavior, and
event-overlay visibility).

## Directory layout

- `mod.rs`
  - Pure directory module: only `pub(super) mod` declarations for the
    three overlay files. No re-exports; `frame.rs` imports each
    render function by name
    (`use super::overlays::{events::render_events_overlay,
    help::render_help_overlay, picker::render_picker_overlay};` —
    `frame.rs:12-13`).
- `picker.rs`
  - Unified bundle+session picker overlay. Side-by-side columns
    (bundles on the left, the active bundle's sessions on the right)
    with a column-scoped filter and per-row readiness styling.
    Opens focused on the session column via `F2` and on the bundle
    column via `F5`. The bundle column drives active-bundle
    switching; selecting a different bundle closes the picker
    overlay and rebinds the active bundle context in the same
    window. Bundle status header is one line of `key=value`
    summary (`bundle.hosted`, `bundle.state`, `bundle.startup_health`,
    reason code) plus capped per-session startup failure lines
    (header `startup_failure_count` carries the true total; see
    `STARTUP_FAILURE_PICKER_MAX_LINES`). The hint strip is generated
    from the binding table via `actions::picker_hint`, filtered to the
    two picker contexts. It is laid out before the vertical split so
    its row count can size its own section — generated wording needs
    more than the one row the hand-written strip fitted in — and it
    packs at entry boundaries rather than wrapping mid-binding. The
    mode-sensitive session label it replaced is gone: the table gives
    one description covering both modes.
  - Every packed row is reserved; there is no row cap. A cap clips
    whichever binding lands last, silently, and the session list's
    `Min(1)` is what should surrender the space instead.
  - Where a single entry cannot fit a row even alone, it degrades to
    the unqualified description (`HelpEntry::detail`) rather than
    being split or clipped. The qualified form is preferred because
    "Bundle col" and "Session col" are what separate the two entries
    that both read as `Enter`; below that width, an ambiguous entry
    beats a missing one.
  - Its one inline `#[cfg(test)]` test is the documented exception,
    on the same grounds as `help.rs`: the renderer is crate-private by
    design and no public interface reaches it. It renders across five
    widths, including those that force a fourth row and the
    unqualified fallback, because the defect it pins was a mismatch
    between the rows the strip reserved and the rows it produced —
    invisible to any test of the packing alone.
- `events.rs`
  - Events overlay. Two stacked panes: pending choices (target,
    kind, `enqueued_at`) and a delivery-events log. Used by `F3`
    from either workbench mode.
- `help.rs`
  - Help overlay. Triggered by `F1` from anywhere. Three columns: two
    of bindings generated from the binding table (`actions/help.rs`)
    and one of reference material. What stays hand-written here is
    what no binding row can hold — the mouse wheel, the predicate
    deciding which interaction pane is live, the `To` address grammar,
    and the keyboard-capability report — declared beside the section it
    annotates. Its one inline `#[cfg(test)]` test is the documented
    exception: the renderer is crate-private by design and no public
    interface reaches it, so the alternative would be a
    render-to-buffer method on `Workbench` existing only for the test.

## Open / close protocol

Each overlay's open flag is owned by `AppState` (`help_overlay_open`,
`picker_open`, `events_overlay_open`) and is toggled by the
`src/tui/state/compose/` helpers. The render layer only reads
these flags. Opening an overlay clears the other two open flags
before flipping its own:

- `open_picker_focused` (`state/compose/pickers.rs:17`) clears
  `events_overlay_open` and `help_overlay_open`.
- `toggle_events_overlay` (`state/compose/pickers.rs:63`) calls
  `close_picker` and clears `help_overlay_open`.
- `toggle_help_overlay` (`state/compose/pickers.rs:72`) calls
  `close_picker` and clears `events_overlay_open`.

These three helpers are the only paths that open an overlay.

## Cross-cutting invariants

- Overlays are drawn in `help` → `picker` → `events` order on top
  of the per-mode main pane; painter's algorithm means a later
  overlay paints over an earlier one.
- Each overlay begins with `frame.render_widget(Clear, popup)` to
  wipe any underlying content within its popup area.
- The picker overlay is the only overlay that paints scrollable
  list content; the others render a static layout.
