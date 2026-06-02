## 1. Contract (spec-only)

- [x] 1.1 Add `Screen Mode Model` requirement: two named modes (Communication,
      Interaction), exactly one active at a time, default = Communication.
- [x] 1.2 Add `Mode Switch Action` requirement: `F4` keybinding, footer
      indicator, preserved-state semantics across switches.
- [x] 1.3 Add `Communication Mode Surface` requirement: panes owned (chat
      history, compose) and behavior carryover from existing workbench
      requirements.
- [x] 1.4 Add `Interaction Mode Surface` requirement: panes owned
      (target/header, look snapshot, raww input, permission decisioning),
      active-target model, empty-target placeholder behavior.
- [x] 1.5 Modify `Session-Scoped Permission Workflow` requirement: replace
      Look-context language with Interaction-mode language; preserve
      filtering/FIFO/empty-state semantics.
- [x] 1.6 Add `Picker Mode-Switch Actions` requirement: picker `l` sets the
      Interaction-mode target and switches mode; picker `w`/`W` sets target +
      focuses raww input (no longer dispatches directly from picker).
- [x] 1.7 Add `Interaction Mode Permission/Raww Pane Replacement` requirement:
      when active target has pending permission requests AND raww input is
      empty, permission pane occupies the raww region; otherwise raww input
      occupies it.
- [x] 1.8 Add `Overlay Availability Across Modes` requirement: picker (F2),
      events (F3), and help (F1) overlays remain available in both modes.
- [x] 1.9 Run `openspec validate refactor-tui-two-mode-surface --strict`
      cleanly.

## 2. Implementation (post-approval)

- [x] 2.1 Introduce `ScreenMode` enum in `state/mod.rs` and a `mode` field on
      `AppState`; remove `look_overlay_open`; add `interaction_target` to
      replace `look_target` with mode-owned lifetime.
- [x] 2.2 Add mode-switch handler in `input.rs` keyed on `F4`; update footer
      to render the mode indicator.
- [x] 2.3 Replace `render_look_overlay` with `render_interaction_mode`; reuse
      existing snapshot/permission rendering helpers but on the main
      `render_main` surface, not a popup.
- [x] 2.4 Add `render_communication_mode` as the existing workbench rendering
      (chat history + compose); top-level `render_main` dispatches by mode.
- [x] 2.5 Add raww input pane in Interaction mode; preserve a per-mode draft
      buffer for raww text (do not share with Communication compose).
- [x] 2.6 Implement raww/permission region replacement: if
      `interaction_target` has pending requests AND raww input is empty,
      render the permission decisioning section; otherwise render the raww
      input.
- [x] 2.7 Retarget picker `l`/`w` actions: set `interaction_target`, switch
      to Interaction mode, focus raww input as appropriate; drop direct raww
      dispatch from picker code path.
- [x] 2.8 Update mode-aware key dispatch in `input.rs`; the existing
      Look-overlay handler becomes the Interaction-mode handler with
      overlay-specific shortcuts removed (no `Esc` closes-overlay because
      mode is not an overlay).
- [x] 2.9 Preserve cursor/draft/scroll state per mode across switches.
- [x] 2.10 Update `src/tui/README.md` Module Map and Current Behavior
      sections; update `documentation/usage/tui.md` key vocabulary section.
      Strip remaining MVP language from these docs project-wide while in
      them.
- [x] 2.11 Add unit/integration coverage for: mode switch toggles cleanly;
      picker actions land in Interaction mode; permission/raww region
      replacement obeys the rule; per-mode state preserved across switches.
- [x] 2.12 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
      `cargo test --all`, `openspec validate --strict` all clean.

## 3. Validation

- [x] 3.1 Operator E2E smoke: launch TUI; verify default Communication mode;
      press `F4`, verify Interaction mode with empty-target placeholder;
      open picker, press `l` on a tmux target, verify Interaction shows look
      snapshot for that session; press `w`, verify raww input is focused;
      clear raww input and produce a pending permission for the target,
      verify permission pane replaces raww region; resolve permission,
      verify raww region returns.
- [x] 3.2 Idle CPU still effectively zero after refactor (regression check
      against issues/tui/8).
