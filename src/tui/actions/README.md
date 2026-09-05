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
- `chord.rs`
  - `Chord` mirrors the condition shapes `../input.rs` uses today (an
    exact modifier set, a key whatever its modifiers, one typed
    character, any typed character), so the table reproduces the
    handlers without narrowing them.
  - The two directions an operator-facing chord travels: `Chord::display`
    renders one for reading, `parse_chord` reads one an operator wrote.
    They live together because they are held to agree — see
    **The operator vocabulary** below.
  - `PrimaryModifier` and `primary_modifier` resolve the symbolic
    `primary` modifier for a platform. The platform arrives as an
    argument rather than through `cfg!`, so both arms are exercisable
    wherever the tests run.
  - A behavior is declared only where it has an effect. Methods that
    guard on the focused field and do nothing elsewhere are reachable
    from a whole screen mode in the handlers; declaring those inert
    rows would preserve no behavior and would make generated help
    offer bindings that do nothing.
- `help.rs`
  - `help_bindings`, which renders the effective table the way the help
    overlay presents it, and `help_contexts`, the presentation rule.
    That rule is deliberately separate from `binding_context` and
    takes no state: help answers for every context, so what it shows
    cannot depend on the surface it was opened from. The effective
    table it renders is a property of the run rather than of the
    surface, so that stays true once a configuration is in play.
  - What a context presents is its configured rows first, then whatever
    of its compiled rows the configuration left standing. Configured
    first because a line's chords fold together and a one-line hint
    strip prints the first of them, so a rebinding that trailed its
    compiled predecessor would be catalogued correctly and still leave
    every strip advertising the chord it replaced. A compiled row drops
    out where a higher tier claimed the keystroke that row is *written*
    as — narrower than the keystrokes it matches, so a row matching a
    key under any modifier keeps answering for the modified forms after
    a configuration takes the bare one.
  - `default_help_bindings` is the one projection that stays on the
    compiled rows, and it takes no effective table rather than
    defaulting one. The usage guide generated from it is committed to
    the repository and read by operators who have written no
    configuration; having nothing to pass is what stops a
    runtime-specific table being documented as everyone's defaults.
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
    bindings instead of on each. Both remain resolvable; only the
    printing folds. The rule is not conditioned on the defaults: an
    entry holds one behavior, so a shown `Enter` on it is one reaching
    the same behavior as the chord being folded into it.
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
- `effective.rs`
  - What an operator configured — `BindingConfiguration`,
    `ConfiguredBinding`, `ConfiguredAction` — and `EffectiveBindings`,
    the table those produce over the compiled defaults.
  - The configured types live here rather than with the file format
    that parses them because they are described in this vocabulary: a
    configured row holds a behavior and a chord, not the strings a file
    spelled them with. That also keeps the dependency between the two
    modules one way. `src/configuration` reads this vocabulary to
    validate a binding group; nothing here reads `src/configuration`.
  - Three tiers answer a lookup, in order: the operator's own rows, the
    rows any named binding set contributed, then the compiled table. The
    first tier holding the chord answers, which is what makes an
    explicit unbinding mean "nothing" rather than deferring downward.
  - The capability class and the platform are arguments to `build`
    rather than probed inside it, so a caller can construct the table
    for either class on either platform without a terminal — and so the
    tests do.
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
`default_help_bindings`, `context_bindings`, `binding_for`,
`typing_binding`, `picker_hint`, `interaction_write_hint`,
`interaction_choice_hint`, `HelpSection`, `HelpEntry`, `HelpSource`,
`parse_chord`, `ChordPattern`, `ChordError`, `PrimaryModifier`,
`primary_modifier`, `BindingConfiguration`, `ConfiguredBinding`,
`ConfiguredAction`, `CapabilityClass`, and `EffectiveBindings` are
exported from `agentmux::tui`, and
`Workbench` exposes `apply_action`, `binding_context`,
`binding_lookup_order`, `bindings`, and `help_bindings`. Together they
let a caller outside the crate ask what surface is active, what a chord
means there, and then invoke it — without naming a chord the TUI
compiled in, and without a `KeyEvent` — or compose the same catalogue
and hint strips the TUI draws, from the table `Workbench::bindings`
hands them.

The presentation functions take an `EffectiveBindings` for that reason:
what they answer is a property of a run, and a caller that has one
should not have to be told which of its rows the TUI would have
honoured. `default_binding` and `default_help_bindings` are the two
that stay on the compiled **defaults**, and neither is a claim that a
chord is fixed — a configuration answers the same question differently,
which is precisely what makes the distinction worth keeping in the
signatures.

What stays internal is the table itself — the rows, their chord
patterns, and their display sections. That is the shape most likely to
move, and nothing outside the crate needs it to ask what a chord does.

## The operator vocabulary

An operator's binding configuration is written by someone who has not
read this source, so the names it uses are a surface of their own
rather than an echo of the Rust identifiers. Two vocabularies carry it,
both kebab-case: `Action::configuration_name` and
`BindingContext::configuration_name`, each with a reverse lookup
derived by searching `ALL` rather than by a second match, so forward
and reverse spellings cannot drift apart.

Both vocabularies are **generated from one declaration** —
`declare_action_vocabulary!` and `declare_binding_contexts!` — which
emit the enum, the `ALL` list, and the names together. That is why a
behavior or context cannot be omitted by oversight: it exists only by
appearing in the declaration, so there is no separate list to fall out
of.

An exhaustive `match` alone would not give that. It forces a new
variant to be *considered*, but nothing forces it into a hand-kept
list, and a behavior missing from that list would carry a name
`from_configuration_name` could never find while every test that walks
the list still passed. The seam is removed rather than policed.

`BindingContext::position` is the deliberate exception: it stays
hand-written and exhaustive rather than derived from `ALL`, because a
position derived from the list it is checked against could never
disagree with it, and the test asserting the two agree is what catches
a context declared in one order and positioned in another.

**Three behaviors are outside the vocabulary**, and stay outside:
`InsertComposeCharacter`, `InsertRawwCharacter`, and
`AppendPickerFilterCharacter` carry the character the operator typed.
They are constructed from a keystroke rather than named in advance, so
a configuration row — which supplies a chord and a name, and never a
character — can neither denote nor build one.
`Action::carries_operator_input` answers for them, and
`configuration_name` answers `None` for exactly those. The two are
separate declarations rather than one derived from the other, and a
test holds them in agreement, so neither can be quietly widened to let
a typing behavior become nameable.

This is the action-side counterpart of excluding `Chord::Text` from the
grammar. The chord side stops an operator from rebinding *how*
characters are typed; the action side stops them from naming *the act
of typing one* as a target.

## What generated help shows is what a configuration accepts

`parse_chord` accepts the spellings `Chord::display` emits, and the two
are held to a round trip: every chord the help overlay presents parses,
and rendering the result reproduces the text that was presented. An
operator's first act is copying a chord out of the reference the TUI
renders, and a grammar that does not accept what help shows fails
exactly then. The test walks the whole generated surface — the chords
drawn on a line and the rows folded out of it, since a folded chord is
one an operator can still press and still wants to rebind.

Two spellings need care, and both are covered:

- `Ctrl+C` is conventionally capitalized, while a terminal reports the
  character as a lowercase `c` and the table stores it that way. Chords
  fold to lower case in `ChordPattern::resolve` rather than at parse
  time, so the rendered text keeps the conventional capital while the
  resolved chord still matches the key that was pressed. Folding at
  resolution also waits until the symbolic modifier is known, so
  `primary+c` folds only where `primary` resolved to `Ctrl`.
- `Shift+Tab` is the one key spelling carrying a modifier in the key
  half, because that is how `BackTab` renders. It parses back to
  `BackTab` bare, not to `Tab` carrying `Shift`: crossterm delivers the
  keystroke as `BackTab` and the compiled rows match on that, so the
  other reading would produce a chord no keystroke ever satisfies.
  `canonical_key` folds the spelling at parse time so one
  representation flows through resolution and dispatch alike.

`Chord::Text` renders as `Type`, which names no key and does not parse.
That is deliberate rather than an omission: admitting it would let a
configuration rebind how characters are typed.

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
  presentation, `tui_binding_vocabulary.rs` for the operator-facing
  names and the chord grammar, and `tui.rs` for the public seam. `src/tui/` carries
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
