## ADDED Requirements

### Requirement: Named Action Vocabulary

The TUI SHALL name every operator-invocable behavior as a member of a single
action vocabulary, distinct from the key chord that invokes it.

Resolving a key chord to an action SHALL be separable from applying an action
to state: applying an action SHALL NOT require a `KeyEvent`, so a host that
owns its own event loop can drive the TUI by action alone.

The action vocabulary SHALL be part of the TUI's public API, and the public
workbench facade SHALL accept and apply an action directly. A host outside the
crate SHALL be able to invoke any operator behavior without constructing a
`KeyEvent` and without reaching crate-private state.

Key-chord resolution SHALL NOT be required to reach action application: a host
that supplies its own bindings SHALL be able to skip resolution entirely.

#### Scenario: An external host applies an action without a key event

- **WHEN** a caller outside the crate applies a named action through the public
  workbench facade
- **THEN** the behavior occurs
- **AND** no `KeyEvent` is constructed to produce it
- **AND** no crate-private type is named by the caller

#### Scenario: Every dispatched chord resolves through the vocabulary

- **WHEN** the TUI handles a key press that invokes a behavior
- **THEN** the chord is first resolved to a named action
- **AND** the behavior is applied from that action

#### Scenario: Key resolution and action application are independently reachable

- **WHEN** a host supplies its own key bindings
- **THEN** it can apply actions without invoking the TUI's chord resolution
- **AND** the resulting behavior matches what the same action produces through
  chord dispatch

### Requirement: Default Binding Table

The TUI SHALL define one binding table mapping (binding context, key chord) to
a named action. That table SHALL be the only place a chord-to-action
association is declared.

The table SHALL define the TUI's **default** bindings. Nothing in the TUI SHALL
require that a chord's action be fixed at compile time: dispatch, presentation,
and documentation SHALL all read the table rather than assuming any particular
chord.

Each row SHALL carry the display section and ordering used to present it, so
presentation is declared with the binding rather than restated elsewhere.

#### Scenario: A chord's action is declared once

- **WHEN** a chord invokes an action in a context
- **THEN** exactly one table row declares that association

#### Scenario: A binding change reaches every consumer

- **WHEN** a table row's chord or action changes
- **THEN** dispatch, the help overlay, and the pane hint strips all reflect the
  change with no other edit

#### Scenario: No consumer hardcodes a chord

- **WHEN** dispatch, help rendering, or hint rendering names a binding
- **THEN** it obtains the chord from the table
- **AND** changing that row changes what the consumer does or shows

### Requirement: Explicit Binding Context Precedence

The TUI SHALL resolve the active binding context from application state as a
single value, rather than as an ordering of handler early-returns.

Overlay contexts SHALL take precedence over screen-mode contexts. Within a
screen mode, the focused field SHALL select the context.

Bindings that hold whichever surface is active SHALL be declared as global rows
in the same table and resolved before the contextual row. Dispatch SHALL NOT
test a chord ahead of the table.

#### Scenario: A global chord is not shadowed by an open surface

- **WHEN** a chord declared in the global rows is pressed while the picker or an
  overlay is open
- **THEN** it invokes the action its global row names
- **AND** no contextual row is consulted for that chord

#### Scenario: An overlay outranks the mode beneath it

- **WHEN** an overlay is open over a screen mode
- **THEN** the resolved binding context is the overlay's
- **AND** chords bound only in the underlying mode do not invoke their actions

#### Scenario: Context resolution is inspectable

- **WHEN** application state is given
- **THEN** the active binding context is obtainable from that state alone,
  without dispatching an event

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
disambiguates modified keys and one that does not.

`Enter` SHALL retain its existing action in every context that has one, and
`Ctrl+J` SHALL retain the insert-newline action in the compose `Message` field
and the interaction write input.

#### Scenario: Modified Enter behaves identically regardless of terminal

- **WHEN** the operator presses `Shift+Enter` or `Ctrl+Enter` in any context
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
- **THEN** the message is sent, as a bare `Enter` sends it

#### Scenario: Newline stays on its own chord

- **WHEN** the operator presses `Ctrl+J` in the compose `Message` field or the
  interaction write input
- **THEN** a newline is inserted
- **AND** no message is sent and no write is dispatched

### Requirement: Generated Operator Binding Documentation

The help overlay, the pane hint strips, and the operator usage guide SHALL be
generated from the binding table, not transcribed from it. No binding string
SHALL be authored outside the table.

Presentation SHALL be governed by a rule separate from dispatch precedence.
The binding context resolved for dispatch selects exactly one context; it SHALL
NOT be used to select which bindings the help overlay presents.

The help overlay SHALL present bindings across every context the operator can
reach, grouped by the display sections declared in the table and ordered by the
table's declaration order. Opening the help overlay SHALL NOT narrow that set:
the reference the operator consults is the same whichever context it was opened
from.

The pane hint strips SHALL instead present only the bindings of the context
they annotate, since a hint strip describes the surface the operator is
currently acting on.

The operator usage guide SHALL contain a generated binding section, and the
repository SHALL fail a lint when that section does not match regeneration from
the table.

#### Scenario: Help presents the whole surface, not the overlay it was opened from

- **WHEN** the operator opens the help overlay from any context
- **THEN** the compose, interaction, and picker bindings are all present
- **AND** the set and order of presented bindings is identical regardless of
  which context the overlay was opened from

#### Scenario: Help grouping follows declared sections

- **WHEN** the help overlay is generated
- **THEN** bindings appear under the display sections declared in the table
- **AND** within a section they appear in the table's declaration order

#### Scenario: A hint strip is scoped to the surface it annotates

- **WHEN** a pane hint strip is generated for a context
- **THEN** it presents only that context's bindings

#### Scenario: Documentation drift fails a lint

- **WHEN** a binding row changes and the usage guide's generated section is not
  regenerated
- **THEN** the repository lint fails
