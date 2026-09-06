## Why

The TUI's chord-to-action associations are compiled. `add-tui-action-layer`
reduced them to one table that dispatch, the help overlay, the pane hint strips,
and the operator usage guide all read, and stated in the table's own module
documentation that terminal classes are meant to diverge through a binding
configuration rather than through the table. That configuration does not exist,
so the operator direction the action layer was built to serve — operators choose
which chords invoke which actions — remains unmet.

The default table is capability-neutral: every modified `Enter` invokes whatever
its context binds to bare `Enter`. That is not a policy anyone selected. Without
the Kitty keyboard protocol a terminal delivers `Shift+Enter` and `Ctrl+Enter`
as the same bytes as bare `Enter`, so the table encodes a technology limitation
rather than a preference. The protocol lifts that limitation on the terminals
that speak it, and today nothing can be done with the lifted limitation, because
the only place a binding can be declared is compiled code shared by every
terminal.

What is missing is a place for an operator to say what their terminal should do
once the limitation is gone. A binding configuration is that place. Divergence
belongs there rather than in shipped defaults: the probe outcome is not
something an operator chose or can see, so a default that varied with it would
make the same binary behave differently on two terminals for reasons invisible
to the person typing.

## What Changes

- **`ui.toml` gains a `[bindings]` group**, declaring rows of (binding context,
  chord, action). Rows are stated in the operator's vocabulary — action names
  and written chords — not in the crate's internal row shape.
- **A configured row may name a different action per terminal capability
  class**: one for a terminal that disambiguates modified keys, one for a
  terminal that does not. A row stating a single value uses it for both, so the
  common case does not pay for the axis. This is the only sanctioned way for a
  binding to vary by terminal capability.
- **The shipped defaults stay capability-neutral.** Both columns ship identical,
  so no binding varies with the probe outcome unless an operator declared that
  it should.
- **Every default row matches exactly what its written form denotes.** Two
  shapes in the compiled table match more than they spell: a control chord
  matches any modifier set containing `Ctrl`, and several rows match a key under
  any modifiers at all. Both were transcribed from the handler conditions the
  action layer replaced, not chosen. They make a configuration incoherent — an
  operator who binds `Ctrl+J` finds `Ctrl+Shift+J` still doing the old thing —
  and they are why the help overlay and dispatch can disagree today. Exact
  matching removes the disagreement at its source rather than teaching each
  consumer to compensate. A modifier variant worth keeping is declared as its
  own row.
- **Named presets** ship as configuration files embedded in the binary and
  parsed by the same parser an operator's configuration goes through, so each is
  both a worked example and a standing proof that the format can express what we
  ship. They carry the binding sets worth adopting wholesale, applied in
  one line rather than transcribed row by row. Two ship: `Enter` inserts a
  newline with `Shift+Enter` sending, and `Enter` inserts a newline with the
  primary-modified `Enter` sending. Both require the protocol, so both declare
  the disambiguating column and contribute nothing under the other. The operator
  names a preset and never a column, so a preset cannot be brought into force
  where it would leave sending unreachable.
- **A symbolic primary modifier** (`primary`) resolves to a literal modifier per
  platform when the effective table is built. `Ctrl` and `Cmd` remain literal
  and separately bindable everywhere, so the seven readline and terminal chords
  the default table carries — `Ctrl+C`, `Ctrl+R`, `Ctrl+A`, `Ctrl+E`, `Ctrl+U`,
  `Ctrl+Space`, `Ctrl+J` — stay correct on macOS, where `Ctrl` is the terminal
  modifier and `Cmd` is the application-command modifier rather than an alias
  for it. Off macOS the symbolic modifier resolves to `Ctrl` unconditionally,
  there being no second application-command modifier for it to choose between; a
  configuration key selects the macOS resolution, which defaults to `Ctrl` too.
- **An action is bindable in a context only where the compiled table already
  declares it.** The table declares a behavior only where it has an effect, so
  deriving the permitted set from its rows keeps a configuration from producing
  a binding that generated help advertises and that does nothing when pressed.
  What a configuration can do is give an existing behavior another chord in a
  context that already declares it; giving a context a behavior it does not have
  needs a compiled-table change, the global context included.
- **Configured rows merge over the compiled defaults row by row.** An operator
  rebinding one action states that row alone and inherits the rest, so an
  override does not silently drop rows added in a later release.
- **Dispatch, help, and the hint strips read an effective table** built from the
  compiled defaults, the configuration, and the probe outcome, rather than the
  compiled table directly. What each consumer does with the table is unchanged.
- **Generated operator documentation states that it documents defaults**, names
  the configuration that supersedes them, and documents the presets.
- **`agentmux check configuration` validates the binding group** through the
  same read-only loader as the rest of `ui.toml`, reporting an unknown action
  name, an action that carries operator input and so cannot be named, an action
  the named context does not declare, an unparseable chord, an unknown binding
  context, an unknown preset, or an unrecognized key, with the physical file in
  effect. It additionally **reports** any behavior the configuration leaves
  unreachable in a context that declares it — which a configuration can do
  without ever unbinding anything, since binding a chord displaces whatever
  action it carried. A report rather than a rejection, because an operator may
  intend it. Pre-flight has no probe outcome and a class-qualified row can be
  reachable under one capability class and not the other, so it builds the
  effective table for each class and names the class every finding holds under.

## Capabilities

### New Capabilities

- `tui-binding-configuration`: the operator-facing binding configuration — its
  place in `ui.toml`, the normative shape of the group, the row grammar for
  contexts, chords, and actions, which actions are nameable at all and in which
  contexts each may be bound, the
  capability columns and how the probe outcome selects between them, the named
  presets and the columns they declare, the symbolic primary modifier
  and its platform resolution, row-level merge over the compiled defaults, and
  the construction and validation of the effective binding table.

### Modified Capabilities

- `tui-action-bindings`: `Default Binding Table` — the compiled table is the
  sole *compiled* declaration and supplies defaults; the effective table
  consumers read is built from it and the configuration.
  `Capability-Neutral Default Bindings` — the requirement stands, with the
  clause governing `Enter`'s retained action scoped to the default table, since
  an operator configuration may now change it.
  `Generated Operator Binding Documentation` — the help overlay and the hint
  strips generate from the effective table; the usage guide generates from the
  defaults and says so.

- `ui-surface-configuration`: `UI Surface Configuration File` — supported fields
  gain the binding group, and pre-flight validation covers it.

## Impact

- `src/tui/actions/bindings.rs` — the compiled table becomes the default layer
  behind an effective table. Its rows keep the actions they invoke and carry no
  capability field, because the shipped defaults do not vary by class. What
  changes is which keystrokes reach them: the shapes that matched more than they
  spelled become exact, and the chord shapes that existed only to reproduce a
  handler condition go away.
- `src/tui/actions/{action,context,help}.rs` — action names and binding contexts
  acquire an operator-facing spelling that the configuration parses and the
  validator reports against.
- `src/configuration/{types,raw,loaders}.rs` — `UiConfiguration` grows the
  binding group; `ui.toml` keeps its existing resolution, absence, and
  malformed-file semantics unchanged.
- `src/tui/{mod,workbench}.rs`, `src/tui/state/mod.rs` — the effective table is
  built where the configuration and the probe outcome are both in hand.
  `AppState::keyboard_enhancement` already carries the probe outcome and
  `KeyboardEnhancement::disambiguates_modified_keys` already predicates on it,
  so the column selector needs no new plumbing into state.
- `documentation/usage/tui.md`, `examples/tui-binding-reference.rs`,
  `scripts/lint-tui-binding-documentation.sh` — the generated section continues
  to render from the defaults, with its standing as defaults made explicit, and
  the guide gains hand-written configuration and preset documentation.
- Operators: every chord the help overlay names keeps its action, and an
  operator who adopts a preset or writes a row gets what they asked for on
  terminals that can deliver it. One change reaches operators who configure
  nothing: a modifier variant the old handler conditions happened to accept, and
  that no row was written as, stops invoking the action it reached by accident.
  `Ctrl+Shift+C` no longer quits; `Alt+Enter` no longer dispatches in the write
  pane. The full set is enumerated rather than estimated, by task 8.1.
- macOS: whether `primary` can usefully resolve to `Cmd` depends on whether a
  terminal delivers `Cmd+Enter` to the process rather than reserving it for its
  own menu shortcuts. No macOS terminal evidence exists in this project yet.
  Because the symbolic modifier reaches only opt-in configuration and one
  preset, no shipped behavior waits on that evidence.
