## ADDED Requirements

### Requirement: Operator Binding Configuration

The runtime SHALL accept operator-declared key bindings from a `[bindings]`
group in `ui.toml`, so which chord invokes which action is a configuration
question rather than a compile-time one.

Within the group, bindings SHALL be declared per binding context, and within a
context each entry SHALL associate one written chord with one named action.
Contexts and actions SHALL be named in kebab-case operator spellings rather than
in the crate's internal identifiers, so the configuration is writable without
reading the source.

Only actions that carry no operator input SHALL be nameable in a configuration.
Actions that carry the operator's own typed character are constructed from the
keystroke rather than from a table row, so a static row can neither name nor
construct one, and naming one SHALL fail validation. The set of nameable actions
SHALL be derivable from the action vocabulary rather than maintained as a
separate list.

Absence of the `[bindings]` group SHALL leave the compiled default bindings in
force, and SHALL NOT be an error.

#### Scenario: A configured chord invokes the configured action

- **WHEN** `ui.toml` declares a chord against an action in a binding context
- **AND** the operator presses that chord on that surface
- **THEN** the named action is invoked

#### Scenario: No binding group leaves the defaults in force

- **WHEN** `ui.toml` declares no `[bindings]` group, or no `ui.toml` is present
  in any configuration layer
- **THEN** every chord invokes the action the compiled default table names
- **AND** startup proceeds

#### Scenario: An action carrying operator input is not nameable

- **WHEN** a configuration names an action that carries the operator's typed
  character
- **THEN** loading fails with a structured validation error naming it

### Requirement: Context and Action Compatibility

A configuration SHALL bind an action within a binding context only where the
compiled table already declares that action in that context, and naming an
action a context does not declare SHALL fail validation.

The compiled table declares a behavior only in the contexts where it has an
effect, so the set of actions it declares for a context is exactly the set that
does something there. Deriving the permitted set from those rows therefore
preserves that rule for configured bindings, rather than restating it as a
separate compatibility table that could disagree with the code.

An operator MAY give a behavior a chord it did not have in any context that
already declares it, the global context included, where a configured row reaches
every surface as the compiled global rows do. Presets SHALL satisfy this
requirement as configured rows do.

The accepted consequence is that introducing a behavior into a context that does
not already declare it requires a change to the compiled table. That holds for
the global context exactly as it holds for every other, so binding an action
globally is not a route around this requirement: the global rows declare their
own small set, and an action outside it is rejected there as anywhere else. This
is the same bar the table itself sets, and it keeps generated help from
advertising a binding that does nothing.

#### Scenario: An action not declared in the context is rejected

- **WHEN** a configuration binds an action in a binding context whose compiled
  rows do not declare that action
- **THEN** loading fails with a structured validation error naming the action
  and the context

#### Scenario: A new chord for an existing contextual behavior is accepted

- **WHEN** a configuration binds a chord to an action the context's compiled
  rows already declare
- **THEN** loading succeeds
- **AND** the effective table reaches that action by both the configured chord
  and the chord the compiled row names

#### Scenario: A global row's action reaches every surface

- **WHEN** a configuration binds a chord to an action the global context's
  compiled rows declare
- **THEN** that action is reachable by that chord from every surface, as global
  rows are

#### Scenario: The global context is not a route around the rule

- **WHEN** a configuration binds an action in the global context whose compiled
  rows do not declare that action
- **THEN** loading fails with a structured validation error
- **AND** the outcome is the same as naming that action in any other context
  that does not declare it

#### Scenario: Generated help advertises no inert configured binding

- **WHEN** the help overlay presents the effective table
- **THEN** every binding it presents invokes an action that has an effect in the
  context it is presented under

### Requirement: Binding Configuration Shape

The `[bindings]` group SHALL have the following shape, so that a configuration
written against this specification loads identically under any conforming
implementation:

```toml
[bindings]
# Binding sets applied before individually configured rows, in the order named.
presets = ["enter-newline-primary-enter-sends"]
# Which literal modifier the symbolic `primary` resolves to on macOS.
primary-modifier-on-macos = "control"

# One sub-table per binding context, keyed by the context's operator name.
[bindings.compose-message]
# A chord mapped to one action applies on both terminal capability classes.
"ctrl+w" = "send-message"
# A chord mapped to a class-qualified table applies only where stated; the
# omitted class keeps its compiled default.
"shift+enter" = { enhanced = "insert-message-newline" }
# An explicitly unbound chord is inert, and does not fall through.
"ctrl+j" = "none"

[bindings.picker-sessions]
"primary+enter" = "commit-picker-session"
```

The two keys directly under `[bindings]` SHALL be `presets`, an ordered list of
preset names, and `primary-modifier-on-macos`, whose permitted values are
`control` and `command`. Both SHALL be optional. Every other key under
`[bindings]` SHALL be a binding context name whose value is that context's
sub-table of chord entries.

A chord entry's value SHALL be either a string naming one action, applying on
both terminal capability classes, or a table whose permitted keys are `enhanced`
and `standard`, each naming one action for that class. The string `none` SHALL
denote an explicit unbinding wherever an action name is permitted.

An unrecognized key at any level SHALL fail validation rather than being
ignored, so a misspelled context or option is reported rather than silently
having no effect.

#### Scenario: The documented shape loads

- **WHEN** a `ui.toml` carrying the shape above is loaded
- **THEN** loading succeeds
- **AND** the effective table carries the preset's bindings, the configured
  rows, and the compiled defaults for every chord not named

#### Scenario: An unrecognized key is a fault

- **WHEN** a configuration carries a key under `[bindings]` that is neither a
  known option nor a known binding context name
- **THEN** loading fails with a structured validation error naming the key

#### Scenario: An out-of-range option value is a fault

- **WHEN** `primary-modifier-on-macos` names a value other than `control` or
  `command`
- **THEN** loading fails with a structured validation error naming the value

### Requirement: Row-Level Merge Over Compiled Defaults

A configured binding SHALL replace only the compiled default sharing its
(binding context, chord) identity, and every default the configuration does not
name SHALL remain in force.

An operator SHALL be able to leave a chord inert in a context by declaring it
against no action, which SHALL NOT fall through to the compiled default for that
chord.

Configuring an action onto a new chord SHALL NOT unbind whatever other chords
reach that action, so an action reachable by two chords is expressible.

#### Scenario: An unnamed default survives an override

- **WHEN** the configuration declares one chord in a context
- **THEN** that chord invokes the configured action
- **AND** every other chord in that context invokes the action its compiled
  default names

#### Scenario: A default added later reaches an existing configuration

- **WHEN** a release adds a compiled default row the operator's configuration
  does not name
- **THEN** that row is in force for that operator

#### Scenario: An explicit unbinding leaves the chord inert

- **WHEN** the configuration declares a chord against no action in a context
- **AND** the operator presses that chord on that surface
- **THEN** no action is invoked
- **AND** the compiled default for that chord is not consulted

#### Scenario: Rebinding an action leaves its other chords standing

- **WHEN** the configuration binds an action to a chord that did not reach it
- **THEN** both that chord and the chord the compiled default names invoke that
  action

### Requirement: Operator Chord Grammar Round-Trips With Presentation

Every chord the help overlay presents that denotes a keystroke SHALL parse as a
configuration chord denoting the chord that was presented, so an operator can
configure a binding by copying the chord out of the reference the TUI itself
renders.

The overlay also presents a placeholder standing for typing rather than for a
keystroke. That placeholder SHALL NOT parse, since accepting it would let a
configuration rebind the rows through which characters are typed, and it is
therefore outside this requirement rather than an exception to it.

The grammar SHALL express a key with an exact modifier set, and SHALL NOT
express the chord shapes that exist to reproduce handler conditions or to carry
typed text. A configuration SHALL NOT be able to rebind the rows through which
characters are typed.

A chord that does not parse SHALL fail validation rather than being ignored.

#### Scenario: A chord copied from help is accepted

- **WHEN** a chord denoting a keystroke is rendered in the help overlay
- **THEN** that written form parses
- **AND** it denotes the same key and modifier set that was rendered

#### Scenario: The typing placeholder does not parse

- **WHEN** the placeholder the overlay renders for a typing row is offered as a
  configuration chord
- **THEN** parsing fails

#### Scenario: Typing rows are not configurable

- **WHEN** a configuration attempts to bind the row through which characters are
  typed
- **THEN** validation fails

#### Scenario: An unparseable chord is a fault

- **WHEN** a configuration declares a chord the grammar does not accept
- **THEN** loading fails with a structured validation error naming the chord
- **AND** the configuration is not partially applied

### Requirement: Terminal Capability Columns

A configured binding SHALL be able to name a different action for a terminal
that disambiguates modified keys than for one that does not, and the runtime
SHALL select between them by the keyboard-enhancement probe outcome.

A binding stated as a single action SHALL apply on both terminal classes. A
binding that names one class only SHALL leave the other class on its compiled
default, so declaring a capability-conditioned chord does not require restating
the behavior it leaves alone.

The capability columns SHALL be the only sanctioned way for a binding to vary by
terminal capability.

#### Scenario: The probe outcome selects the column

- **WHEN** a configured binding names an action per terminal class
- **AND** the keyboard enhancement probe reports the protocol active
- **THEN** the action named for the disambiguating class is invoked

#### Scenario: A single value applies to both classes

- **WHEN** a configured binding names one action without naming a class
- **THEN** that action is invoked whatever the probe reports

#### Scenario: An omitted class keeps its compiled default

- **WHEN** a configured binding names an action for one terminal class only
- **AND** the probe reports the other class
- **THEN** the compiled default for that chord is in force

### Requirement: Named Binding Presets

The runtime SHALL provide named binding presets that an operator applies by
name, so a binding set worth adopting wholesale is adopted in one line rather
than transcribed row by row and left to drift as the default table grows.

A preset SHALL declare the terminal capability class its bindings apply to, and
SHALL contribute nothing where the keyboard-enhancement probe reports a
different class. The operator names a preset and never a class, so a preset
whose bindings depend on modified keys being distinguishable cannot be brought
into force where they are not — the restriction is structural rather than a
check the operator can fail.

Presets SHALL be applied in the order named, so a later preset binding the same
(binding context, chord) as an earlier one supersedes it.

Presets SHALL compose with individually configured rows, and a configured row
naming the same (binding context, chord) as a preset row SHALL take precedence
over it, so adopting a preset does not prevent adjusting one of its rows.

Two presets SHALL ship, both declaring the disambiguating class: one binding
`Enter` to insert a newline with `Shift+Enter` sending, and one binding `Enter`
to insert a newline with the primary-modified `Enter` sending.

A shipped preset SHALL be expressed in the same configuration format an operator
writes, embedded in the binary, and SHALL be parsed by the same parser that
accepts an operator's configuration, rather than having its rows constructed
directly in code. Its rows SHALL be obtained by parsing that embedded text when
the preset is first needed.

A repository check SHALL parse every shipped preset, so a preset that does not
parse fails the repository's checks rather than reaching a release. Because the
text is fixed at compile time and the parser is the same code the check
exercises, a shipped preset failing to parse at run time is an internal
invariant violation rather than an operator-facing configuration fault, and it
SHALL NOT be reported as a fault in the operator's configuration file.

#### Scenario: Shipped presets are expressed in the operator's own format

- **WHEN** a shipped preset supplies its bindings
- **THEN** those bindings are obtained by parsing the embedded configuration text
- **AND** the same parser accepts it that accepts an operator's configuration

#### Scenario: A shipped preset that does not parse fails the repository checks

- **WHEN** a shipped preset cannot be parsed
- **THEN** the repository's checks fail

#### Scenario: A shipped preset's parse failure is not blamed on the operator

- **WHEN** a shipped preset fails to parse at run time
- **THEN** the failure is not reported as a fault in the operator's
  configuration file

#### Scenario: A preset supplies its bindings

- **WHEN** the configuration names a preset
- **AND** the probe reports the capability class the preset declares
- **THEN** the effective table carries that preset's bindings

#### Scenario: A later preset supersedes an earlier one

- **WHEN** the configuration names two presets binding the same chord in the
  same context
- **THEN** the binding from the later-named preset is in force

#### Scenario: A configured row overrides a preset row

- **WHEN** the configuration names a preset and also declares a row for a
  (binding context, chord) the preset binds
- **THEN** the configured row is in force for that chord

#### Scenario: A preset leaves the other capability class alone

- **WHEN** the configuration names a preset declaring the disambiguating class
- **AND** the probe reports that modified keys are not distinguishable
- **THEN** the compiled default bindings are in force

### Requirement: Symbolic Primary Modifier

The chord grammar SHALL provide one symbolic modifier that resolves to a
platform-appropriate literal modifier when the effective table is built, so a
binding meant to follow the platform is declared once.

`Ctrl` and a macOS `Command` modifier SHALL remain literal and separately
bindable on every platform. The symbolic modifier SHALL NOT be implemented as a
rewrite over literal `Ctrl` bindings, because on macOS the two modifiers coexist
carrying different meanings and the terminal and readline chords the default
table declares are genuinely `Ctrl` there.

On every platform other than macOS the symbolic modifier SHALL resolve to
`Ctrl`. On macOS a configuration key SHALL select which literal modifier it
resolves to, and where that key is absent the symbolic modifier SHALL resolve to
`Ctrl` there as well, so a chord using it is reachable on every platform without
depending on evidence this project does not yet hold.

#### Scenario: The symbolic modifier resolves to Ctrl off macOS

- **WHEN** a configured binding uses the symbolic modifier
- **AND** the running platform is not macOS
- **THEN** the effective table carries that binding under `Ctrl`
- **AND** the macOS selection key has no effect on the resolution

#### Scenario: The symbolic modifier resolves to Ctrl on macOS by default

- **WHEN** a configured binding uses the symbolic modifier on macOS
- **AND** the configuration selects no macOS resolution
- **THEN** the effective table carries that binding under `Ctrl`

#### Scenario: Literal control bindings are untouched by the symbolic modifier

- **WHEN** the default table's literal `Ctrl` bindings are resolved on macOS
- **THEN** each remains bound to `Ctrl`
- **AND** none is rewritten to the `Command` modifier

#### Scenario: The macOS resolution is operator-selectable

- **WHEN** the configuration selects which literal modifier the symbolic
  modifier resolves to on macOS
- **THEN** the effective table on macOS carries that modifier for symbolic
  bindings

### Requirement: Effective Binding Table Construction

The TUI SHALL resolve chords against an effective binding table built from the
compiled defaults, the operator configuration, and the keyboard-enhancement
probe outcome, and every consumer that reads bindings at run time SHALL read
that table.

Within a binding context, configured rows SHALL be resolved before compiled
rows, so a configured chord is not shadowed by a compiled row that would also
match it.

The precedence between binding contexts SHALL be unchanged: global rows SHALL
still resolve before the contextual surface, so a configured contextual row does
not shadow a global one.

#### Scenario: A configured row is not shadowed by a broader compiled row

- **WHEN** a compiled row in a context would also match a configured chord
- **THEN** the configured row resolves

#### Scenario: A configured contextual row does not shadow a global row

- **WHEN** the configuration binds a chord in a contextual surface
- **AND** a compiled global row declares the same chord
- **THEN** the global row resolves on that surface

#### Scenario: Run-time consumers reflect the configuration

- **WHEN** a binding is configured
- **THEN** the help overlay presents the configured chord
- **AND** a pane hint strip that advertises that binding presents it too

### Requirement: Binding Configuration Validation

The runtime SHALL reject a binding configuration that names an unknown action,
an unknown binding context, an unknown preset, or an unparseable chord, failing
with a structured validation error rather than applying it partially or
ignoring it silently.

Loading SHALL reject a configuration under which no chord reaches the action
that quits the TUI under **either** capability class, since that condition is
not recoverable from inside the running application and the class a given
operator's terminal falls into is not knowable when the configuration is
written. Rejecting on either class rather than on the active one keeps the
answer the same at startup, where the probe outcome is known, and at pre-flight,
where it is not. Quit is the only action whose loss meets that bar: every other
binding can be restored by quitting and editing the configuration.

Losing a binding does not require an explicit unbinding. Binding a chord that
already carried an action displaces that action, so a configuration can leave a
behavior unreachable in a context without ever naming it — rebinding the compose
`Message` field's `Ctrl+J` drops insert-newline there, silently.

`agentmux check configuration` runs outside a TUI session and so has no
keyboard-enhancement probe outcome, while a class-qualified row can leave an
action reachable under one capability class and unreachable under the other.
There is therefore no single effective table for pre-flight to inspect.
Pre-flight SHALL construct the effective table for **each** capability class and
inspect both. Where a class's effective table leaves an action unreachable in a
context whose compiled rows declare it, pre-flight SHALL report the action, the
context, and the capability class under which it is unreachable. A finding
holding under both classes SHALL say so rather than being reported twice.

Reporting SHALL be a report rather than a rejection, because an operator may
intend it; declaring the chord against no action is how they say so
deliberately, which is what distinguishes an intended removal from a
displacement. The report describes the outcome and SHALL NOT attempt to
distinguish the two.

#### Scenario: A displaced action is reported by pre-flight

- **WHEN** a configuration binds a chord that a compiled row in that context
  bound to another action
- **AND** no other chord in that context reaches the displaced action under
  either capability class
- **THEN** pre-flight reports that action and context as unreachable under both
  classes
- **AND** loading succeeds

#### Scenario: A finding holding under one capability class names that class

- **WHEN** a class-qualified configured row leaves an action unreachable in a
  context under one capability class only
- **THEN** pre-flight reports that action, that context, and that class
- **AND** it does not report the action as unreachable under the other class

`agentmux check configuration` SHALL validate the binding group through the same
read-only loader and the same effective-file lookup as the rest of `ui.toml`,
reporting the physical file the lookup selected so an operator can tell which
configuration layer is at fault.

#### Scenario: An unknown action name is a fault

- **WHEN** a configuration names an action the TUI does not define
- **THEN** loading fails with a structured validation error naming it
- **AND** no binding from that configuration is applied

#### Scenario: A configuration that cannot quit is rejected

- **WHEN** a configuration leaves no chord reaching the quit action under either
  capability class
- **THEN** loading fails with a structured validation error

#### Scenario: Quit unreachable under one class alone is still rejected

- **WHEN** a class-qualified configured row leaves no chord reaching the quit
  action under one capability class, while the other class retains one
- **THEN** loading fails with a structured validation error naming that class

#### Scenario: Pre-flight names the layer of an invalid binding group

- **WHEN** `agentmux check configuration` reports an invalid binding group
- **AND** more than one configuration layer supplies a `ui.toml`
- **THEN** the reported path is the copy in effect rather than any copy it
  shadows
