# TUI Actions

This directory names what the operator can ask the workbench to do,
separately from the key chords that ask for it. Splitting the two is
what lets a host drive the workbench without synthesizing terminal
events, and what gives dispatch, help rendering, and the pane hint
strips one declaration to read instead of several transcriptions.

Two halves, deliberately independent:

- **Resolution** turns a chord plus the current state into an
  `Action`. It needs the binding context and the binding table.
- **Behavior** applies an `Action` to `AppState`. It needs neither a
  `KeyEvent` nor a binding context, so a host that supplies its own
  bindings can skip resolution entirely.

## Directory layout

- `mod.rs`
  - Pure hub: submodule declarations and the re-exports.
- `action.rs`
  - The `Action` enum — one member per operator-invocable behavior,
    derived from the handlers in `../input.rs` — and `Action::apply`,
    which performs it. Application calls `AppState` methods rather
    than reaching its fields, so a later regrouping of the state
    struct does not have to rewrite this layer.
  - `Action` is public and re-exported from `agentmux::tui`; the
    public seam that applies one is `Workbench::apply_action`.
- `bindings.rs`
  - The default chord-to-action table, and `default_binding`, which
    reads it. Rows are grouped under their context rather than
    repeating it, and within a group declaration order is the tiebreak
    between rows that could both match — so a row for one character
    must precede its context's typing row.
  - `Chord` mirrors the condition shapes `../input.rs` uses today (an
    exact modifier set, a key whatever its modifiers, one typed
    character, any typed character), so the table reproduces the
    handlers without narrowing them.
  - A behavior is declared only where it has an effect. Methods that
    guard on the focused field and do nothing elsewhere are reachable
    from a whole screen mode in the handlers; declaring those inert
    rows would preserve no behavior and would make generated help
    offer bindings that do nothing.
- `help.rs`
  - `help_bindings`, which renders the whole table the way the help
    overlay presents it, and `help_contexts`, the presentation rule.
    That rule is deliberately separate from `binding_context` and
    takes no state: help answers for every context, so what it shows
    cannot depend on the surface it was opened from.
  - The help-overlay context contributes rows to two display sections:
    the mode-switching chords it shares with every surface, and a
    `Help Overlay` section of its own holding the chords that move the
    overlay's viewport. They are filed apart because the first group
    reaches other surfaces while the second moves what is drawn of the
    surface the operator is already on.
  - Entries group by behavior rather than by chord, so a context's
    several `Enter` rows fold onto one line. Capability-neutral
    defaults make `Shift+Enter` and `Ctrl+Enter` redundant once
    `Enter` is on that line, so they are stated once beside the
    bindings instead of on each. Both remain resolvable through
    `default_binding`; only the printing folds.
  - Every row is recorded in `HelpEntry::sources`, folded or not, with
    the context that declared it and `HelpSource::matches`, which
    answers whether that row is one that answers a given key. That
    provenance is what lets a test check presentation against the table
    rather than against a copy of it. Both halves are load-bearing: the
    context catches a surface dropped from `help_contexts`, whose
    behaviors some other context still describes; the pattern catches a
    single row dropped from a context that binds two chords to one
    behavior, as the events overlay does with `Esc` and `F3`.
  - `context_bindings`, `binding_for`, and `typing_binding` answer for
    one context rather than the whole surface, and `picker_hint`,
    `interaction_write_hint`, and `interaction_choice_hint` compose the
    pane hint strips from them. That asymmetry is the point: help
    catalogues every surface, a strip annotates the one it sits on.
    Which few behaviors a strip advertises stays declared here; their
    chords and wording are the table's. `HelpEntry::primary_chord` and
    `HelpEntry::detail` are what a one-line strip uses, the catalogue
    using the full chord list and the qualified description.
  - The choice pane's strip is the short one, carrying its two
    decisions and not its four navigation rows. It prints in a block
    title, which does not wrap, so what a wider strip would gain in
    completeness it loses to the pane edge. Where a strip's own
    medium constrains it, the constraint is declared here rather than
    solved by trimming the wording every consumer shares.
- `context.rs`
  - `BindingContext`, `binding_context`, and `binding_lookup_order`.
    `binding_context` resolves the surface that owns a chord from
    `AppState` alone: overlay surfaces outrank screen-mode surfaces,
    and within a mode the focused field selects the surface.
    `binding_lookup_order` puts `BindingContext::Global` ahead of that
    surface, so a chord bound globally is not shadowed by whatever is
    open over it.

## Public surface

`Action`, `BindingContext`, `default_binding`, `help_bindings`,
`context_bindings`, `binding_for`, `typing_binding`, `picker_hint`,
`interaction_write_hint`, `interaction_choice_hint`, `HelpSection`,
`HelpEntry`, and `HelpSource` are exported from `agentmux::tui`, and
`Workbench` exposes `apply_action`, `binding_context`,
`binding_lookup_order`, and `help_bindings`. Together they let a caller
outside the crate ask what surface is active, what a chord means there,
and then invoke it — without naming a chord the TUI compiled in, and
without a `KeyEvent` — or render its own binding reference from
`help_bindings` and `Action::describe`.

`default_binding` reports the compiled **defaults**. It is not a claim
that a chord is fixed: operator-configured bindings are the intended
successor to this table, and a configuration is expected to answer the
same question differently.

What stays internal is the table itself — the rows, their chord
patterns, and their display sections. That is the shape most likely to
move, and nothing outside the crate needs it to ask what a chord does.

## Global rows

`Ctrl+C` and `F1` reach their behaviors from every surface today,
because `handle_key` tests them before it consults any overlay. That
reach could not survive as an early return: a chord tested ahead of the
table is a second place a chord-to-action association is declared,
which is the duplication this directory exists to remove. They are
rows under the global context instead, and dispatch walks
`binding_lookup_order` rather than special-casing them.

## Reproducing the handlers rather than tidying them

The handlers are not uniform about modifiers, and the table matches
them as they are:

- The interaction and picker arms test `KeyCode::Enter` whatever the
  modifiers, so those contexts carry a fallback row after their three
  explicit `Enter` rows. Without it `Alt+Enter` would go inert. Compose
  carries no fallback, because its arm guards on `modifiers.is_empty()`.
- The control blocks test `modifiers.contains(CONTROL)`, so a control
  chord matches however the modifiers are combined. `Ctrl+Shift+J`
  reaches the same behavior as `Ctrl+J`.

The three explicit `Enter` rows own the modifier sets the
capability-neutrality contract governs. The fallback exists so the
other sets keep their current behavior, not to weaken that contract.
Presentation folds a context's several `Enter` rows onto one line and
then folds the modified forms out of it entirely, since neutrality
makes them redundant wherever `Enter` is bound; see `help.rs`.

## Notes

- `Action` carries the operator's own input where a behavior needs it
  (the inserted character), never a chord. A chord never appears in
  the vocabulary.
- Paste and mouse events stay event-shape concerns in `../input.rs`
  and have no action members.
- Tests live under `tests/unit/`: `tui_bindings.rs` for what the table
  declares, `tui_dispatch.rs` for dispatch and direct application
  agreeing on what a chord does, `tui_help.rs` for generated
  presentation, and `tui.rs` for the public seam. `src/tui/` carries
  four inline `#[cfg(test)]` blocks, one `#[test]` each, all covering
  crate-private renderers no public interface reaches:
  `../render/overlays/help.rs` for the overlay's geometry and its
  independence from the keyboard probe,
  `../render/overlays/picker.rs` for the hint strip surviving widths
  that force extra rows, `../render/interaction.rs` for the choice
  pane's title showing a whole advertised binding or none of it, and
  `../render/frame.rs` for the footer and the startup status naming
  the chords the table declares. Everything else lives under `tests/`.
- The usage guide is a fourth consumer, and the only one that cannot
  read the table at render time. `examples/tui-binding-reference.rs`
  renders the guide's generated section from `help_bindings` and
  nothing else, and `scripts/lint-tui-binding-documentation.sh` fails
  the commit when the committed section and the regeneration disagree.
  That the example reaches the table through the public exports alone
  is the point: it compiles the claim that a caller outside the crate
  can build its own binding reference.
