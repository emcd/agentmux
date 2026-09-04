## Context

`add-tui-action-layer` left the TUI with one compiled table associating
(binding context, chord) with a named action, and with every consumer —
dispatch, the help overlay, the pane hint strips, the generated usage guide —
reading it rather than transcribing it. The table's module documentation names
this change as its successor and states the constraint it was written under:
rows carry no capability field, because nothing may vary by keyboard-enhancement
probe outcome while the table is the only declaration site.

That neutrality encodes a property of terminals rather than a policy. Without
the Kitty keyboard protocol, `Shift+Enter` and `Ctrl+Enter` reach the process as
the same bytes as bare `Enter`; the table cannot distinguish them because the
terminal did not. The protocol lifts that limitation where it is active, and
this change supplies the place an operator says what to do with the lifted
limitation.

Two properties of the existing configuration subsystem govern how bindings can
be introduced. Files resolve through an ordered layer list where the first layer
supplying a file wins, and **replacement is whole-file, not key-merging**
(`src/configuration/README.md`). And `ui.toml` already exists as the
operator-surface preferences file, documented as designed to grow further
surface keys, with its resolution, absence, and malformed-file semantics already
specified in `ui-surface-configuration`.

The probe outcome is already in application state: `AppState` carries
`keyboard_enhancement`, and `KeyboardEnhancement::disambiguates_modified_keys`
is the predicate over it. Nothing new has to reach state for a capability column
to be selectable.

## Goals / Non-Goals

**Goals:**

- Operators declare which chords invoke which actions, without rebuilding.
- Terminal-capability variance has a declaration site, so an operator whose
  terminal lifts the limitation can act on it.
- The binding sets worth adopting wholesale are applied in one line rather than
  transcribed row by row.
- The platform axis is carried by the format from the outset, since retrofitting
  an axis into a shipped file format is expensive.
- An operator who states one row keeps every row they did not state, including
  rows added in later releases.
- What the help overlay prints as a chord is what the configuration accepts as
  a chord.

**Non-Goals:**

- Changing what the TUI does out of the box on any terminal.
- Reloading bindings without restarting the TUI. The effective table is built
  once at startup.
- Rebinding the typing rows, or any chord shape that exists to reproduce a
  handler rather than to be operator-facing.
- Per-bundle, per-session, or per-agent binding sets. One effective table.
- Changing what any action *does*. Only which chord reaches it.
- Resolving the macOS delivery question. The format carries the axis, and
  nothing shipped depends on the answer.

## Decisions

### The shipped defaults stay capability-neutral; divergence is opt-in

Both capability columns ship identical. The idiom that the protocol makes
possible — `Enter` inserting a newline, a modified `Enter` sending — is offered
as a preset rather than imposed as a default.

*Alternative considered: shipping the idiom as the default for terminals that
disambiguate.* Rejected, and the reasoning is worth recording because the
opposite conclusion is easy to reach. The probe outcome is not something the
operator selected, and it is not visible on the surface where they are typing:
`SendMessage` is advertised on no rendered pane today, only in the help overlay
behind `F1`. A default varying on that discriminator makes the same binary,
under the same configuration, send on `Enter` in one terminal and insert a
newline in another, for a reason the operator cannot see. Opt-in divergence has
none of that property, because the operator who diverges chose it and knows
they did. The argument that neutrality is itself surprising — that `Shift+Enter`
sends where chat clients insert a newline — measures surprise against other
applications rather than against what agentmux does everywhere else, and the
latter is the baseline an operator actually carries between terminals.

The consequence is that this change alters no observable behavior for anyone,
which is also what makes it safe to land before the macOS question is settled.

### The compiled default table needs no capability field

Because both columns ship identical, the statement in the table's module
documentation stands unchanged: rows carry no capability field, and a per-row
flag would be unused machinery. Capability scoping lives on configured rows and
on presets, which is where variance is actually declared.

### Presets ship as configuration files, parsed by the operator's own parser

A preset is a named set of rows applied by naming it. Each preset declares the
capability class its rows apply to, and contributes nothing where the probe
reports a different class. The operator names a preset and never a class, so a
preset requiring the protocol cannot be brought into force where the protocol is
absent — there is no way to express the misapplication, which is better than
detecting it.

A shipped preset is written in the configuration format an operator writes and
embedded in the binary, rather than constructed in code. That makes each shipped
preset a conformance test of the format: if the format cannot express what we
want to ship, we find out at our own build rather than after an operator asks
for something the grammar cannot say. It also makes every preset a worked
example that cannot drift from the grammar, which generalizes the single fixture
test from one documented shape to every set we ship.

The rows come from parsing that embedded text when a preset is first needed. An
earlier draft of this decision said the parse must not happen at run time, which
was an overreach with no artifact behind it: a repository test and an embedded
string do not by themselves hand the runtime any rows, so something has to parse
them. The goal was never to avoid a parse — it was that a malformed shipped
preset must never reach an operator, and a repository check that parses every
shipped preset delivers exactly that.

What remains is deciding what a run-time parse failure *means*. The text is
fixed at compile time and the parser is the same code the check exercises, so
failure implies a defect in our own artifact, not in anything the operator did.
It is an invariant violation, and the one thing it must not do is present itself
as a fault in the operator's configuration file, which would send them auditing
a file that is fine.

*Alternative considered: a build script generating validated rows from the TOML,
so no run-time parse exists.* Rejected as disproportionate. The repository has
no `build.rs` today, and introducing the build-time codegen path to eliminate a
parse whose input is a compile-time constant buys an invariant the test already
guarantees.

*Alternative considered: documenting copy-paste blocks in the usage guide.*
Rejected: a snippet an operator pasted last year is not revised when the table
grows a row, and nothing validates it. A named set stays correct as the table
evolves, is checked before release, and is one line to adopt or drop.

The `Enter`-is-unreachable hazard is thereby designed out rather than checked
for. An earlier draft made it a validation rule, which was a guard on a
condition the format gives the operator no way to produce.

### Bindings live in `ui.toml`, not a file of their own

`ui.toml` is the operator-surface file, and its type documentation already
anticipates growth. Adding a `[bindings]` group reuses its resolution, its
absence rule, its malformed-file rule, and its pre-flight coverage without
restating any of them.

*Alternative considered: a separate `bindings.toml`.* It would give bindings
their own shadowing granularity — an operator overriding bindings in an early
layer would not also take over `default-bundle`, which under whole-file
replacement they otherwise must restate. That coupling is real and is accepted
below as a trade-off. It was not decisive because the whole-file rule is uniform
across every configuration file *by design* — "overriding one is the same
operation regardless of which file it is" — and introducing a file specifically
to obtain finer granularity argues against the layer model rather than fitting
it. `ui.toml` carries one other key today, so the cost of restating it is one
line. If binding configurations grow until the coupling hurts, splitting them
out is a later change with evidence behind it.

### A row is identified by (context, chord), and merges over the compiled defaults

The configuration answers the same question the table answers: what does this
chord do in this context. A configured row replaces the compiled row with the
same identity and leaves every other row standing.

*Alternative considered: identity by (context, action)* — "send is now on this
chord". Rejected because moving an action would require the loader to remove a
row the operator never named, which is action at a distance; and because two
chords reaching one action is a thing operators legitimately want, which an
action-keyed format cannot express. The consequence of chord-keyed identity is
that rebinding an action to a new chord leaves the old chord still reaching it
until the operator unbinds it explicitly, which is visible in the help overlay
and is usually the desired outcome.

*Alternative considered: whole-table replacement*, matching the file-level
whole-file rule. Rejected: the default table carries dozens of rows, so every
override would become a standing maintenance liability that silently drops rows
added in later releases. The two rules govern different axes and do not
conflict — whole-file replacement decides *which layer's `ui.toml` is read*, and
row-level merge decides how that file's contents relate to *compiled* defaults.
The design states this explicitly because the two can be misread as
contradictory.

Unbinding is explicit: a row whose action is `none` leaves the chord inert in
that context rather than falling through to the compiled row.

### The grammar expresses one chord shape, and it round-trips with help

`Chord` carries five variants. Only `Key(code, modifiers)` — a key with an exact
modifier set — is operator-facing. `AnyModifiers` and `Control` exist to
reproduce handler arms that test a key code or a `CONTROL` membership without
narrowing them; `Char` and `Text` are typing rather than binding. Exposing any
of the four would ask operators to reason about fidelity artifacts, and exposing
`Text` would let a configuration make typing impossible.

The written form is the one `Chord::display` already produces, and the parser
and that renderer are held to round-trip: **any chord the help overlay prints
parses back to the chord it printed**. An operator's first act is copying a
chord out of help; a grammar that does not accept what help shows fails at
exactly that moment. This is a testable invariant over the whole table rather
than a spot check, and it constrains future display changes to stay parseable.

### Only actions carrying no operator input are nameable

`Action` carries the operator's own typed character in three variants —
`InsertComposeCharacter`, `InsertRawwCharacter`, and
`AppendPickerFilterCharacter`. A static configuration row supplies a chord and
an action name, not a keystroke, so it can neither name nor construct one of
these. Naming one is a validation failure rather than a silently inert row.

The configurable subset is *derived* from the vocabulary rather than kept as a
hand-maintained list. A list would be a second place the distinction is
declared, and a variant added later would land on whichever side the list's
author forgot — which is the same class of defect the binding table itself
exists to remove. A test asserts that every data-carrying variant is outside the
subset, so the derivation is checked rather than assumed.

This is the action-side counterpart of excluding `Chord::Text` from the grammar:
the chord side stops an operator from rebinding *how* characters are typed, and
the action side stops them from naming *the act of typing one* as a target.

### What an action may be bound to is derived from the compiled rows

A configuration may bind an action in a context only where the compiled table
already declares it there. The table declares a behavior only where it has an
effect — `ClearToField` reaches `state.clear_to_field()`, which does nothing
outside the `To` field, so no other context declares it — which means the
compiled rows already encode "has an effect here". Deriving the permitted set
from them reuses that invariant instead of introducing a compatibility matrix
that could disagree with the code.

*Alternative considered: permitting any input-free action in any context.* That
was the draft's behavior, and it would let a configuration reintroduce exactly
what the table's rule exists to prevent: a binding that generated help
advertises and that does nothing when pressed. The draft's own first example
tripped over it.

The accepted cost is that giving a context a genuinely new behavior needs a code
change — the same bar the table sets for the project. That cost is not avoidable
by binding globally: the compiled global rows declare only `Quit` and
`ToggleHelpOverlay`, so the global context is subject to the rule like any
other, and an action outside its two rows is rejected there too. An earlier
draft of this decision offered global binding as an escape hatch for behaviors a
surface does not declare, which the rule three sentences above it would have
made a validation failure.

What an operator can do without a code change is give an existing behavior
another chord, in any context that already declares it — including a second
chord for `Quit` or the help overlay globally.

### The configuration shape is normative, and a fixture proves it

The specification carries a complete worked `[bindings]` group rather than
fragments, because a file format described only by its constraints admits
several mutually incompatible loaders. A fixture test loads that documented
shape verbatim and asserts the effective table it produces, so the published
shape and the accepted shape cannot drift apart.

Unrecognized keys fail rather than being ignored, at every level. A misspelled
context name that is silently skipped produces a configuration the operator
believes is in force and that does nothing — the same failure mode as a shadowed
configuration layer, which the configuration module already treats as
unacceptable.

### Configured rows resolve ahead of compiled rows within their context

Declaration order is already the tiebreak within a context. Configured rows
precede compiled ones, so a configured `Shift+Enter` is not shadowed by the
compiled `AnyModifiers(Enter)` fallback its context carries.

`binding_lookup_order` — global rows before the contextual surface — is
unchanged. A configured contextual row therefore does not shadow a compiled
global one: changing `Ctrl+C` means configuring the global context, not the
compose one. That keeps a globally reachable chord globally reachable, which is
the property the global rows exist to hold.

### Capability is a per-row value, not a pair of sub-tables

A row states either one action, used on both terminal classes, or a table
naming `enhanced` and `standard` separately:

```toml
[bindings.compose-message]
"enter" = { enhanced = "insert-message-newline" }
"primary+enter" = { enhanced = "send-message" }
"ctrl+w" = "send-message"
```

The classes are named for the capability rather than for the mechanism that
supplies it or for the act of detecting it. `detected` was the first spelling
and said nothing about *what* was detected; `kitty-protocol` was the second and
named a mechanism our own dependency abstracts away — crossterm's probe is
Kitty-specific today, but its API is `KeyboardEnhancement`, and a public file
format should not bind itself more tightly to a mechanism than the code does.
`standard` rather than `legacy` for the other class, because it is the majority
case and the safe default rather than something on its way out.

*Alternative considered: `[bindings.enhanced]` and `[bindings.standard]`
sub-tables.* Rejected because it forces every row to be stated twice to say the
same thing, while the overwhelmingly common case is a row that does not vary at
all. The per-row form makes the axis cost proportional to the number of rows
that actually use it.

A column stated for one class and omitted for the other leaves the omitted class
on its compiled default, which is what makes the two lines above mean "only on a
capable terminal" without restating the behavior they leave alone.

### `primary` is a symbolic modifier; its macOS resolution is a named value

`Ctrl` and `Cmd` stay literal and separately bindable everywhere. One symbolic
modifier, `primary`, resolves per platform at effective-table build time. On macOS
`Ctrl` is the terminal and readline modifier while `Cmd` is the
application-command modifier; they coexist meaning different things, which is
why a global `Ctrl`→`Cmd` rewrite would break the seven readline and terminal
chords the default table carries — `Ctrl+C`, `Ctrl+R`, `Ctrl+A`, `Ctrl+E`,
`Ctrl+U`, `Ctrl+Space`, and `Ctrl+J`. `Ctrl+C` quit would become `Cmd+C` copy,
and quit would be unreachable.

Off macOS the symbolic modifier resolves to `Ctrl`, unconditionally and with no
key governing it — there is no second application-command modifier on those
platforms for it to choose between. The knob exists only because macOS has two,
and it is spelled as a value rather than a boolean:

```toml
[bindings]
primary-modifier-on-macos = "control"   # or "command"
```

Absent the key, the macOS resolution is also `Ctrl`, so a chord using the
symbolic modifier is reachable everywhere without waiting on evidence this
project does not hold.

*Alternative considered: the boolean form* (`mod-is-control-on-macos`, or the
inverse). A boolean encodes the default in its polarity, so changing the default
later inverts what every operator's existing `true` means. The default here is
exactly the thing that is unverified, which makes polarity the worst property
this key could have. A two-valued key has no polarity to invert. This is a
spelling change to the knob the operator agreed to, not a change to its scope,
but it is called out for review because it deviates from what was agreed.

### The effective table is a value, built once at startup

Resolution stops being answerable by a free function over compiled data. The
effective table is constructed where the configuration and the probe outcome are
both in hand, and held by the workbench. `Action` application is already
independent of resolution, so only the resolution half changes; a host that
supplies its own bindings is unaffected.

Documentation generation keeps reading the *default* table: the generated
reference in `documentation/usage/tui.md` documents what the TUI does before any
configuration, which is the only thing it can truthfully document.

## Risks / Trade-offs

- **A configuration can lock the operator out of quitting** → validation rejects
  a configuration under which no chord reaches the quit action, reporting the
  file in effect. Preferred over a hidden compiled escape hatch, which would
  contradict the property that the table is what dispatch reads. Quit is the
  only action meeting that bar, since every other loss is repaired by quitting
  and editing the file — which is precisely what quit's loss prevents.
- **A behavior can go missing without the operator ever unbinding it** →
  binding a chord that already carried an action displaces that action, so
  rebinding the compose `Message` field's `Ctrl+J` drops insert-newline there
  with nothing said about it. `agentmux check configuration` reports any action
  the compiled rows declare in a context that an effective table leaves
  unreachable there. A report rather than a rejection, because the operator may
  mean it — and declaring the chord against `none` is how they say so, which is
  what separates an intended removal from an accident.
- **Pre-flight has no probe outcome, and there is no single effective table** →
  `agentmux check configuration` runs outside a TUI session, while a
  class-qualified row can leave an action reachable under one capability class
  and not the other. Pre-flight therefore builds the effective table for each
  class and reports the class each finding holds under. The quit rejection takes
  the conservative form of the same rule — unreachable under *either* class is a
  rejection — so that the answer does not depend on whether a probe outcome
  happened to be available when the question was asked.
- **An operator adopting a preset changes their send chord with no on-surface
  indication**, since no pane advertises `SendMessage` → the help overlay
  reflects it, and the operator opted in knowingly, which is the difference
  between this and a default that varies. That no pane advertises the send chord
  is a pre-existing gap rather than one this change introduces; it is recorded
  separately rather than solved here.
- **A preset that requires the protocol could come into force where the protocol
  is absent**, leaving sending unreachable → not expressible. A preset declares
  the class its rows apply to and contributes nothing under any other, and the
  operator names only the preset.
- **macOS `Cmd+Enter` may never reach the process**, because macOS terminals
  routinely reserve `Cmd` chords for their own menus → nothing shipped depends
  on the answer, since the symbolic modifier reaches only opt-in configuration
  and one preset. The macOS resolution ships as `control` and flips only when
  `todos/tui/62` reports evidence.
- **Overriding `ui.toml` in an early layer takes over `default-bundle` too**,
  under whole-file replacement → accepted, and inherent to the uniform layer
  rule rather than to this change. One line restates it.
- **Chord-keyed merge leaves the old chord bound when an action moves** →
  visible in the help overlay, and removable with an explicit `none` row.
- **The grammar's round-trip invariant constrains future display changes** →
  intended. A chord that help can print but the configuration cannot accept is
  the defect the invariant exists to prevent.

## Migration Plan

Nothing to migrate. No operator has a binding configuration today, the shipped
defaults are unchanged in both capability columns, and every terminal observes
exactly the behavior it observes now. An operator opts in by naming a preset or
writing a row.

Rollback is granular: the format, the parser, the presets, and the effective
table are independent of one another. Withdrawing a preset leaves the
configuration mechanism in place.

## Open Questions

- Does a macOS terminal deliver `Cmd+Enter` to the process at all, and if so
  does crossterm report `SUPER` distinctly under the Kitty protocol? The two
  are ordered — the second is only worth asking where the first says yes.
  `todos/tui/62` is the place to gather it. This governs the shipped resolution
  of `primary` on macOS, which reaches only opt-in configuration.
- Should the binding-set selector be spelled `keybindings` rather than
  `presets`? Operator-definable named sets are deferred to `todos/tui/67`, and
  under that design a shipped set and an operator's set are one concept. Naming
  the selector for the concept now costs a word; renaming a shipped format key
  later does not.
- Should further presets ship, beyond the two the operator named? Adding one
  later is cheap; the question is only whether any other set is common enough to
  be worth naming.
