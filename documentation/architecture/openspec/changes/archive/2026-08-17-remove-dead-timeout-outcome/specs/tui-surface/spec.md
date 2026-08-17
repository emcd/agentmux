## MODIFIED Requirements

### Requirement: TUI Delivery State Mapping

TUI state and history surfaces SHALL use this outcome vocabulary:

- `accepted`
- `success`
- `failed`
- `not_submitted`
- `submission_unknown`

Mapping rules SHALL be:

- async send acceptance maps to `accepted`
- terminal delivered outcome maps to `success`
- terminal delivery failure maps to `failed`
- terminal relay state `dropped_on_shutdown` maps to `failed` with
  `reason_code=dropped_on_shutdown`
- terminal `not_submitted` maps to `not_submitted`
- terminal `submission_unknown` maps to `submission_unknown`

`not_submitted` and `submission_unknown` SHALL be represented distinctly rather
than collapsed into `failed` or into an unknown-outcome placeholder. They carry
opposite evidentiary claims — one asserts that no target-side effect occurred,
the other that such an effect cannot be excluded — and a surface that renders
either as a failure asserts a non-delivery the relay cannot support.

`accepted` is process-local state derived from send acknowledgement and SHALL
NOT require replay after reconnect.

A terminal outcome the TUI does not recognize SHALL be surfaced as an explicit
unknown-outcome placeholder rather than mapped to any of the above.

#### Scenario: Represent async send acceptance as accepted

- **WHEN** relay accepts an async send request for one or more targets
- **THEN** TUI records initial delivery state `accepted` for those targets

#### Scenario: Transition accepted state to terminal outcome

- **WHEN** TUI receives terminal delivery update for an accepted target
- **THEN** TUI updates state to exactly one of `success`, `failed`,
  `not_submitted`, or `submission_unknown`

#### Scenario: Represent an evidence-bearing terminal outcome distinctly

- **WHEN** TUI receives a terminal delivery outcome of `not_submitted` or
  `submission_unknown` from a relay stream event
- **THEN** TUI records that outcome under its own spelling
- **AND** does not render it as `failed` or as an unknown-outcome placeholder

#### Scenario: Treat accepted state as local on reconnect

- **WHEN** TUI reconnects stream handling after process restart
- **THEN** TUI does not require replay of `accepted` lifecycle events
- **AND** applies terminal outcomes from relay stream events
