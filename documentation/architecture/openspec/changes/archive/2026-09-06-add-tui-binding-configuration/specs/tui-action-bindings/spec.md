## MODIFIED Requirements

### Requirement: Default Binding Table

The TUI SHALL define one binding table mapping (binding context, key chord) to
a named action. That table SHALL be the only place in compiled code where a
chord-to-action association is declared.

The table SHALL define the TUI's **default** bindings, which an operator
configuration supersedes row by row as the `tui-binding-configuration`
capability governs. Nothing in the TUI SHALL require that a chord's action be
fixed at compile time: dispatch and presentation SHALL read the effective table
built from these defaults and the operator configuration, rather than assuming
any particular chord.

Table rows SHALL carry no terminal-capability qualifier, since the defaults do
not vary by capability class. Capability variance SHALL be expressed where it is
declared — in an operator configuration or in a named preset.

Each row SHALL carry the display section and ordering used to present it, so
presentation is declared with the binding rather than restated elsewhere.

A table row SHALL match exactly the keystrokes its written form denotes, and no
others. A row naming a key with a modifier set SHALL NOT match that key under
any other set. A row naming a key with no modifiers SHALL NOT match it under any
modifier, except where that key is a single character, which the paragraph below
governs. A modifier variant intended to invoke an action SHALL be declared as
its own row.

A chord naming a single character with no modifiers is the one written form
denoting two keystrokes: that character bare, and that character carrying
`Shift`, and no other. A terminal's report of a typed character is not a
function of the key alone — `Shift` and `Caps Lock` each alter both which
character is reported and which modifiers accompany it — so a form admitting
only one of the two would refuse a keystroke an operator produced by typing.
Between them, `c` and `C` so defined cover every way either letter can arrive.

That denotation SHALL be the same on both sides. A compiled row naming a bare
character and an operator's chord naming that character SHALL match the same two
keystrokes, whether the row carries the character into an action or invokes a
fixed one. Claiming the character in a configuration therefore claims all of
what the compiled row answered, leaving nothing behind on the form the
configuration did not spell out.

Symmetry is the requirement, not the pair. A written form that denoted more on
the compiled side than an operator's spelling of it denotes would be a row a
configuration cannot fully claim, which is the condition the rest of this
requirement exists to remove — reintroduced for one shape rather than avoided.

Exactness is what lets dispatch, presentation, and reachability agree without
coordinating. A row matching more than it spells is one a configuration cannot
fully claim, so the behavior it carries survives being rebound, the help overlay
and dispatch disagree about whether it is still bound, and no configuration can
give the unclaimed variants a different meaning.

#### Scenario: A keystroke outside the written form's denotation reaches nothing

- **WHEN** a table row's written form denotes a set of keystrokes
- **AND** a keystroke outside that set arrives
- **THEN** the row does not match it
- **AND** the keystroke reaches whatever other row denotes it, or nothing

#### Scenario: A rebound chord takes its behavior with it

- **WHEN** a configuration binds the chord a table row is written as
- **THEN** no keystroke reaches that row's action through that row
- **AND** the help overlay and dispatch agree that it no longer does

#### Scenario: A shifted character still reaches its row

- **WHEN** a character arrives carrying `Shift`
- **THEN** the row naming that character accepts it
- **AND** this holds whether the row carries the character into an action or
  invokes a fixed one

#### Scenario: Configuring a character claims both of its forms

- **WHEN** a configuration binds a chord naming a single character with no
  modifiers
- **AND** that character arrives carrying `Shift`
- **THEN** the configured action is invoked
- **AND** the compiled row that named the same character reaches nothing

#### Scenario: A chord's action is declared once

- **WHEN** a chord invokes an action in a context
- **THEN** exactly one table row declares that association

#### Scenario: A binding change reaches every consumer

- **WHEN** a table row's chord or action changes
- **THEN** dispatch, the help overlay, and the pane hint strips all reflect the
  change with no other edit

#### Scenario: No consumer hardcodes a chord

- **WHEN** dispatch, help rendering, or hint rendering names a binding
- **THEN** it obtains the chord from the effective table
- **AND** changing that row changes what the consumer does or shows

### Requirement: Capability-Neutral Default Bindings

Every binding context that binds `Enter` SHALL declare its `Shift+Enter` and
`Ctrl+Enter` rows explicitly, and a context that binds no `Enter` action SHALL
declare none of the three, leaving all three forms equally inert there. A
context SHALL NOT acquire a modified-`Enter` behavior by omitting a modifier
condition.

In the default table, `Shift+Enter` and `Ctrl+Enter` SHALL each invoke the same
action their context binds to `Enter`.

No default binding SHALL vary with the keyboard-enhancement probe outcome. The
default table SHALL produce identical observable behavior on a terminal that
disambiguates modified keys and one that does not. This requirement governs the
defaults; an operator configuration MAY declare a binding that varies by
capability class, as the `tui-binding-configuration` capability governs, and
doing so SHALL NOT be constrained by this requirement.

In the default table, `Enter` SHALL retain its existing action in every context
that has one, and `Ctrl+J` SHALL retain the insert-newline action in the compose
`Message` field and the interaction write input.

#### Scenario: Modified Enter behaves identically regardless of terminal

- **WHEN** the operator presses `Shift+Enter` or `Ctrl+Enter` in any context
- **AND** no operator binding configuration is in force
- **THEN** the action invoked is the one that context binds to `Enter`, and no
  action is invoked in a context that binds no `Enter` action
- **AND** the observable result does not depend on whether the terminal
  disambiguates modified keys

#### Scenario: A context cannot inherit modified Enter by omission

- **WHEN** a binding context declares an `Enter` row
- **THEN** its `Shift+Enter` and `Ctrl+Enter` rows are declared explicitly
- **AND** an undeclared modified chord invokes no action

#### Scenario: Send remains reachable by modified Enter on a capable terminal

- **WHEN** the terminal disambiguates modified keys and the operator presses
  `Shift+Enter` in the compose `Message` field
- **AND** no operator binding configuration is in force
- **THEN** the message is sent, as a bare `Enter` sends it

#### Scenario: Newline stays on its own chord

- **WHEN** the operator presses `Ctrl+J` in the compose `Message` field or the
  interaction write input
- **AND** no operator binding configuration is in force
- **THEN** a newline is inserted
- **AND** no message is sent and no write is dispatched

#### Scenario: A configured divergence is outside this requirement

- **WHEN** an operator configuration declares a binding that varies by terminal
  capability class
- **THEN** the effective table carries that variance
- **AND** the default table remains capability-neutral

### Requirement: Generated Operator Binding Documentation

The help overlay, the pane hint strips, and the operator usage guide SHALL be
generated from the binding table, not transcribed from it. No binding string
SHALL be authored outside the table in any surface of this project that tells an
operator what a chord currently does.

Operator-authored configuration lies outside that prohibition by construction:
declaring a binding is what the `tui-binding-configuration` capability exists to
permit. Hand-written documentation MAY show configuration syntax by example,
provided the example illustrates the syntax rather than asserting what the
default bindings are, which the generated section owns.

The help overlay and the pane hint strips SHALL generate from the **effective**
table, so what they present is what the operator's configuration and terminal
actually produce. The operator usage guide SHALL generate from the **default**
table, since it is authored before any operator configuration exists, and it
SHALL state that the bindings it documents are defaults an operator
configuration supersedes.

Presentation SHALL be governed by a rule separate from dispatch precedence.
The binding context resolved for dispatch selects exactly one context; it SHALL
NOT be used to select which bindings the help overlay presents.

The help overlay SHALL present bindings across every context the operator can
reach, grouped by the display sections declared in the table and ordered by the
table's declaration order. Opening the help overlay SHALL NOT narrow that set:
the reference the operator consults is the same whichever context it was opened
from.

The set the overlay presents SHALL NOT be narrowed by the size of the terminal
either. Where the terminal cannot show the whole presentation at once, the
remainder SHALL remain reachable, as the `Reachable Help Presentation`
requirement governs.

The pane hint strips SHALL instead present only the bindings of the context
they annotate, since a hint strip describes the surface the operator is
currently acting on.

The operator usage guide SHALL contain a generated binding section, and the
repository SHALL fail a lint when that section does not match regeneration from
the default table.

#### Scenario: Help presents the whole surface, not the overlay it was opened from

- **WHEN** the operator opens the help overlay from any context
- **THEN** the compose, interaction, and picker bindings are all present
- **AND** the set and order of presented bindings is identical regardless of
  which context the overlay was opened from

#### Scenario: Help grouping follows declared sections

- **WHEN** the help overlay is generated
- **THEN** bindings appear under the display sections declared in the table
- **AND** within a section they appear in the table's declaration order

#### Scenario: Help presents the operator's configured chords

- **WHEN** an operator configuration rebinds an action
- **THEN** the help overlay presents the configured chord for that action
- **AND** where the surface carries a hint strip that advertises that action,
  the strip presents the configured chord too

#### Scenario: The usage guide documents defaults and says so

- **WHEN** the operator usage guide's generated binding section is rendered
- **THEN** it presents the default bindings
- **AND** it states that an operator configuration supersedes them

#### Scenario: A hint strip is scoped to the surface it annotates

- **WHEN** a pane hint strip is generated for a context
- **THEN** it presents only that context's bindings

#### Scenario: Documentation drift fails a lint

- **WHEN** a binding row changes and the usage guide's generated section is not
  regenerated
- **THEN** the repository lint fails

### Requirement: Explicit Binding Context Precedence

The TUI SHALL resolve the active binding context from application state as a
single value, rather than as an ordering of handler early-returns.

Overlay contexts SHALL take precedence over screen-mode contexts. Within a
screen mode, the focused field SHALL select the context.

Bindings that hold whichever surface is active SHALL be declared as global rows
in the same table and resolved before the contextual row. Dispatch SHALL NOT
test a chord ahead of the table.

The order the contexts are consulted in SHALL be declared once and read by
every consumer of it. Dispatch is not the only consumer: asking what a
configuration leaves reachable asks the same question of every surface, and a
consumer that restated the order could answer differently from the one that
resolves keystrokes.

Resolution SHALL take the first consulted context that binds the chord to an
action. A context that binds the chord to no action SHALL NOT halt resolution:
an explicit unbinding empties the chord in the context that named it and
uncovers whatever the next consulted context binds, which is the same scoping
every row has. Emptying a global chord therefore reveals a surface row it was
shadowing rather than silencing the key everywhere.

#### Scenario: A global chord is not shadowed by an open surface

- **WHEN** a chord bound to an action in the global rows is pressed while the
  picker or an overlay is open
- **THEN** it invokes the action its global row names
- **AND** no contextual row is consulted for that chord

#### Scenario: An emptied global chord uncovers the surface beneath it

- **WHEN** a configuration binds a global chord to no action
- **AND** the active surface binds that chord
- **THEN** the surface's action is invoked
- **AND** the chord is not silenced on surfaces that do not bind it

#### Scenario: An overlay outranks the mode beneath it

- **WHEN** an overlay is open over a screen mode
- **THEN** the resolved binding context is the overlay's
- **AND** chords bound only in the underlying mode do not invoke their actions

#### Scenario: Context resolution is inspectable

- **WHEN** application state is given
- **THEN** the active binding context is obtainable from that state alone,
  without dispatching an event

#### Scenario: One declaration of the consultation order

- **WHEN** the contexts consulted for a chord are enumerated
- **THEN** dispatch and any other consumer read the same declaration
- **AND** changing that declaration changes both
