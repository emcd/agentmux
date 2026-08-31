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
  - Pure hub: submodule declarations and the `Action` re-export.
- `action.rs`
  - The `Action` enum — one member per operator-invocable behavior,
    derived from the handlers in `../input.rs` — and `Action::apply`,
    which performs it. Application calls `AppState` methods rather
    than reaching its fields, so a later regrouping of the state
    struct does not have to rewrite this layer.
  - `Action` is public and re-exported from `agentmux::tui`; the
    public seam that applies one is `Workbench::apply_action`.
- `bindings.rs`
  - The default chord-to-action table, declared per binding context.
- `context.rs`
  - `BindingContext`, `binding_context`, and `binding_lookup_order`.
    `binding_context` resolves the surface that owns a chord from
    `AppState` alone: overlay surfaces outrank screen-mode surfaces,
    and within a mode the focused field selects the surface.
    `binding_lookup_order` puts `BindingContext::Global` ahead of that
    surface, so a chord bound globally is not shadowed by whatever is
    open over it. All three stay crate-private: embedding needs the
    action vocabulary, not the TUI's own precedence model.

## Global rows

`Ctrl+C` and `F1` reach their behaviors from every surface today,
because `handle_key` tests them before it consults any overlay. That
reach has to survive the move to a table, and it cannot survive as an
early return: a chord tested ahead of the table is a second place a
chord-to-action association is declared, which is the duplication this
directory exists to remove. So they become rows under the global
context, and dispatch walks `binding_lookup_order` rather than
special-casing them.

## Notes

- `Action` carries the operator's own input where a behavior needs it
  (the inserted character), never a chord. A chord never appears in
  the vocabulary.
- Paste and mouse events stay event-shape concerns in `../input.rs`
  and have no action members.
- `context.rs` holds the only inline `#[cfg(test)]` block in
  `src/tui/`: `binding_context` is crate-private by design, and no
  public interface exercises it, so testing it externally would mean
  widening visibility that would itself become API surface.
