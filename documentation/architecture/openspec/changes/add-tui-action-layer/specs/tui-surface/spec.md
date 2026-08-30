## MODIFIED Requirements

### Requirement: Keyboard Enhancement Capability Detection

The TUI SHALL probe the terminal once for progressive keyboard enhancement
(the Kitty keyboard protocol) after terminal setup and before the event loop
reads any key event, so the probe's query reply is not consumed by the loop's
input drain.

The probe SHALL resolve to exactly one of three outcomes:

- `Active` — the terminal advertised the protocol and the TUI pushed
  `DISAMBIGUATE_ESCAPE_CODES`,
- `Unsupported` — the terminal answered the probe without advertising the
  protocol,
- `ProbeFailed` — the probe did not complete (no controlling terminal, I/O
  failure, or no reply before the query timeout).

`ProbeFailed` SHALL be reported distinctly from `Unsupported`; the TUI SHALL
NOT collapse an unanswered probe into a negative answer.

The TUI SHALL push only `DISAMBIGUATE_ESCAPE_CODES`. It SHALL NOT push flags
that change which events are delivered (`REPORT_EVENT_TYPES`,
`REPORT_ALTERNATE_KEYS`, `REPORT_ALL_KEYS_AS_ESCAPE_CODES`) while no input
handler consumes them.

Pushed flags SHALL be popped before the terminal is restored, so the terminal
is left in the key-reporting mode it had before launch.

The TUI SHALL activate disambiguation where the terminal offers it so that
modified chords are deliverable and therefore bindable, widening the space of
chords the binding table can name. Activation SHALL NOT by itself change what
any chord does.

The probe outcome SHALL be visible to the operator in the help overlay, because
it reports what the TUI was able to determine: that disambiguation is active,
that the terminal answered without offering it, or that the determination could
not be made. The report SHALL describe the determination, never assert a
terminal limitation the probe did not establish.

The report for `ProbeFailed` SHALL NOT assert that the terminal lacks the
protocol. An unanswered probe establishes only that the TUI could not
determine or enable disambiguation.

Detection SHALL NOT itself introduce, remove, or reassign any key binding; it
bears only on whether a chord is deliverable, never on what a delivered chord
does. Which action a delivered chord invokes SHALL be declared per context in
the binding table, so no context acquires a modified-`Enter` behavior by
omitting a modifier condition.

The default bindings SHALL be capability-neutral: the observable result of any
chord SHALL NOT depend on the probe outcome. A modified `Enter` delivered
distinctly under `Active` SHALL invoke the same action it invokes under
`Unsupported` and `ProbeFailed`, where it is physically indistinguishable from
a bare `Enter`.

`Ctrl+J` SHALL insert a newline under every outcome.

#### Scenario: Modified Enter behaves the same under every probe outcome

- **WHEN** the operator presses `Shift+Enter` in the `Message` field
- **THEN** the message is sent
- **AND** the result is the same whether the protocol is active, unsupported, or
  the probe failed

#### Scenario: Activation alone changes no behavior

- **WHEN** the protocol is active and disambiguation flags are pushed
- **THEN** every chord invokes the action it invokes without disambiguation
- **AND** the operator observes no behavior difference attributable to the probe

#### Scenario: Probe failure claims nothing about the terminal

- **WHEN** the probe does not complete
- **THEN** the operator-facing report states that the capability is
  undetermined
- **AND** it does not state that the terminal lacks the protocol

#### Scenario: Capable terminal activates disambiguation

- **WHEN** the terminal advertises progressive keyboard enhancement at TUI
  startup
- **THEN** the TUI pushes `DISAMBIGUATE_ESCAPE_CODES`
- **AND** the help overlay reports the protocol as active
- **AND** the flags are popped before the terminal is restored

#### Scenario: Terminal answers without advertising support

- **WHEN** the terminal answers the probe without advertising the protocol
- **THEN** the TUI pushes no enhancement flags
- **AND** the help overlay reports the protocol as unsupported

#### Scenario: Unanswered probe is distinct from an unsupported terminal

- **WHEN** the keyboard-enhancement probe does not complete
- **THEN** the outcome is reported as a probe failure
- **AND** that report is distinguishable from the report for a terminal that
  answered without advertising support

#### Scenario: Newline binding is unchanged by the probe outcome

- **WHEN** the operator inserts a newline in `Message` or the write input
- **THEN** `Ctrl+J` inserts the newline regardless of the probe outcome
