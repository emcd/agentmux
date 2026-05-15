# Change: Refactor TUI to Two Co-Equal Screen Modes (Communication / Interaction)

## Why

The current TUI is a single "workbench" surface (header + chat history + compose
+ footer) with operator workflows bolted on as overlays (picker, events, look,
help). Two structural problems followed from this single-surface model:

- The pending-permission decisioning UI had nowhere natural to live. It was
  first added as a workbench-bottom pane (truncating compose) and then moved
  into the Look overlay, conditional on the Look target having pending
  requests. Both placements are compromises.
- Inspection-shaped workflows (look snapshot, raww dispatch, permission
  decisions) and communication-shaped workflows (chat, compose) are mixed in
  the same surface even though they have different focus models, key
  vocabularies, and visual needs (snapshot density vs. chat history
  scrollback).

The operator's intent is two co-equal top-level screen modes — Communication
and Interaction — that the operator toggles between, with each mode owning its
own panes and key vocabulary, instead of one mode owning the surface and the
other living in overlays.

This is a TUI-surface refactor; it changes no relay, MCP, or ACP contract. It
does change the contract between operator and TUI for which workflow lives on
which surface.

## What Changes

- Define two top-level TUI screen modes that are peers:
  - **Communication** — owns send/receive: chat history, compose (To +
    Message), pending-delivery indicator, send dispatch. Today's chat history
    and compose panes.
  - **Interaction** — owns operator-driven session inspection: Look snapshot,
    raww dispatch input, and permission decisioning. Replaces the current
    full-screen Look overlay and the picker `w` raww dispatch.
- Replace the current full-screen Look overlay with a mode (Interaction). Look
  snapshot becomes the dominant pane of Interaction mode. The overlay shape
  (Clear + popup over workbench) is removed.
- Replace the picker `w`/`W` raww action with a dedicated raww input pane
  inside Interaction mode. The raww pane is part of the mode, not a transient
  state.
- Pending-permission decisioning lives inside Interaction mode (not in Look or
  in the workbench). When the active interaction target has pending permission
  requests and the raww input is empty, the permission decisioning pane
  replaces the raww input pane in the same screen region. When the raww input
  is non-empty, raww keeps the region. **BREAKING (TUI UX)**: the
  workbench-bottom permission pane and the Look-embedded permission section
  are both removed.
- Add a deterministic mode-switch key binding (`F4`, with footer indicator).
  Mode state is process-scoped and preserved across switches (Communication
  retains compose draft + scroll position; Interaction retains active target +
  look scroll + raww draft + permission selection cursor).
- Picker (F2), events overlay (F3), and help (F1) remain overlays in both
  modes; they overlay whichever mode is active.
- Keep all permission, raww, and look relay contracts unchanged. The active
  Interaction-mode target uses the same `look_target` semantics as today's
  Look overlay.
- **BREAKING (TUI UX)**: picker `l` no longer opens a full-screen Look
  overlay; it sets the Interaction-mode target and switches to Interaction
  mode. Picker `w`/`W` no longer dispatches raww directly; it sets the
  Interaction-mode target and focuses the raww input.

## Impact

- Affected specs:
  - `tui-surface` (primary; mode model, Interaction-mode requirements,
    permission-pane placement, mode-switch key contract, picker mode-switch
    actions, Session-Scoped Permission Workflow language)
- Affected code:
  - `src/tui/render.rs` — split rendering by mode; remove Look overlay; add
    Interaction-mode layout (target/look-snapshot/raww-or-permission)
  - `src/tui/input.rs` — mode-aware key dispatch; mode-switch handler;
    remove Look-overlay key handler shape; move raww-dispatch trigger from
    picker to Interaction mode
  - `src/tui/state/mod.rs` — add `mode: ScreenMode` field; replace
    `look_overlay_open` with mode + active interaction target
  - `src/tui/state/compose.rs` — retarget `raww_picker_target` callers to
    Interaction-mode raww submit; preserve raww draft per mode
  - `src/tui/README.md`, `documentation/usage/tui.md` — doc refresh; strip
    remaining MVP language as part of this slice
- Not affected:
  - relay, MCP, ACP contracts and event payloads,
  - permission lifecycle / decision contract (`permission.resolve`),
  - delivery outcome vocabulary,
  - sender/bundle identity precedence.
