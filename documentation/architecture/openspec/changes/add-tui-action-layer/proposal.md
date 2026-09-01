## Why

`src/tui/input.rs` fuses the key, the context condition, and the state
mutation into every match arm: 66 `KeyCode::` arms across six handler
functions, with no action vocabulary and no binding table. The binding
knowledge is then restated in prose everywhere else, with nothing tying the
copies together — `Ctrl+J` is bound twice in `input.rs` (once per mode) and
restated in twelve more places across the help overlay, two pane hint strips,
the usage guide, the TUI README, and the `tui-surface` spec. The help overlay
is a hand-maintained transcript of a match statement, and nothing detects when
they diverge.

The Kitty keyboard capability detection that landed on the TUI lane turned
that latent cost into a live defect. Of the three `KeyCode::Enter` arms, only the
`Communication` arm guards on `modifiers.is_empty()`; `Interaction` and the
picker do not. On a terminal that disambiguates, `Shift+Enter` is therefore
silently dropped in `To`/`Message` while it still dispatches the write and
still commits the picker selection — an inconsistency nobody chose, produced
by which match arm happened to omit a guard. Enabling disambiguation makes the
input surface *less* uniform until a grammar names modified `Enter`
deliberately.

## What Changes

- Introduce a named `Action` vocabulary, exported from the TUI's public API,
  and give the public `Workbench` facade direct action application. Key-to-action
  resolution becomes separable from action-to-behavior, so an embedding host can
  drive the TUI with actions instead of synthesising `KeyEvent`s
  (`ideas/tui/1`). This is new public surface, not an internal refactor: today
  `Workbench` exposes only `dispatch_event` and `AppState` is crate-private, so
  no host outside the crate can invoke a behavior by name.
- Introduce a context-scoped binding table as the single source of truth for
  which chord invokes which action in which context (compose `To`, compose
  `Message`, interaction write, interaction choice pane, picker columns,
  overlays, global).
- Declare modified `Enter` per context instead of inheriting it, with
  **capability-neutral defaults**: `Shift+Enter` and `Ctrl+Enter` each invoke
  the same action their context binds to `Enter`. Every context that binds
  `Enter` must declare all three rows, and a context binding no `Enter` action
  must declare none of them, so no context can acquire a behavior by omitting a
  modifier guard.
- **No operator-visible behavior change on any terminal.** The neutral defaults
  also repair a regression: activating disambiguation made `Shift+Enter` in
  `Message` stop sending on capable terminals, because the compose handler
  rejects a modified `Enter`.
- No binding varies with the keyboard-enhancement probe outcome. Capability-
  conditioned bindings are a **configuration** question — a binding
  configuration with two value columns, one per terminal class — and are
  deliberately not compiled in. The table here defines defaults, so that
  configuration is an additive change later rather than a contradiction of this
  one.
- `Ctrl+J` keeps the insert-newline action in both multi-line fields, unchanged.
- Generate the help overlay and both pane hint strips from the binding table.
  Help presentation is governed by a rule separate from dispatch precedence —
  the overlay presents the whole reachable surface, while hint strips stay
  scoped to the context they annotate.
- Add a repository lint that fails when `documentation/usage/tui.md` and the
  binding table disagree, replacing hand-synchronisation with a gate.

Operator-configurable bindings are the **intended successor** to this change,
not part of it. Landing a config parser first would leave the five-way
duplication this change exists to remove; landing the table first makes the
configuration additive. The same applies to embeddable widgets: this change
delivers the input seam they need, and is shaped so the two remaining seams —
a public render entry taking a caller-supplied `Rect`, and decomposing
`AppState` — stay cheap to add.

## Capabilities

### New Capabilities

- `tui-action-bindings`: the action vocabulary, the context-scoped binding
  table, capability-neutral default bindings, the separation of key-to-action
  resolution
  from action-to-behavior, and the guarantee that operator-facing binding
  documentation is generated from the table rather than transcribed.

### Modified Capabilities

- `tui-surface`: modified-`Enter` semantics become a per-context declaration
  with capability-neutral defaults. The `Keyboard Enhancement Capability
  Detection` requirement currently records the incidental split between the
  guarded `Communication` arm and the unguarded `Interaction`/picker arms as
  observed behavior; that paragraph and its two modified-`Enter` scenarios are
  replaced by the statement that the probe outcome changes no chord's
  observable result. Existing `Enter` behavior is unchanged in every context.

## Impact

- `src/tui/input.rs` — the six handler functions become action dispatch over a
  resolved action; the match arms lose their embedded key conditions.
- `src/tui/workbench.rs` and the `agentmux::tui` exports — new public surface:
  the `Action` type and an action-application method alongside the existing
  `dispatch_event`, plus `BindingContext` and the `default_binding` lookup that
  answers what a chord means on a given surface. The table itself stays
  internal; its rows are the shape most likely to move, and nothing outside the
  crate needs them in order to ask what a chord does.
- `src/tui/render/overlays/help.rs` — 49 hardcoded `Line::from` binding strings
  are replaced by generation from the table.
- `src/tui/render/overlays/picker.rs` (`picker_hint_line`) and
  `src/tui/render/interaction.rs` (the write-pane hint) — both already
  hand-roll context-sensitive action labels; both become table consumers.
- `src/tui/keyboard.rs` — unchanged. The probe outcome keeps its existing
  consumer (operator reporting) and gains no influence over dispatch.
- `documentation/usage/tui.md` — becomes lint-checked against the table.
- No relay, transport, or protocol surface is touched. The mailbox delivery
  redesign wires `src/transports/ui.rs`, which does not touch `src/tui/`; the
  two are separated by the relay stream boundary at `src/tui/state/relay.rs`.
- Prerequisite satisfied: the `todos/tui/22` capability detection has landed,
  so modified chords are deliverable and therefore bindable.
- Successors this shapes rather than blocks: `todos/tui/64` (operator-
  configurable bindings with two-column capability values), `todos/tui/65` and
  `ideas/tui/1` (embeddable widgets, which need action-based input), and
  `todos/tui/63` (`AppState` decomposition, the other widget blocker).
