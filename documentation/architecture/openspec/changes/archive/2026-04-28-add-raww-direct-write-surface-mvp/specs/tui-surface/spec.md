## ADDED Requirements

### Requirement: TUI raww dispatch contract

TUI raw write actions SHALL dispatch through relay raww contract and SHALL NOT
perform transport-specific writes directly.

TUI raww requests SHALL include:
- `target_session`
- `text`
- optional `no_enter` (default `false`)

#### Scenario: Dispatch raww through relay contract

- **WHEN** operator triggers raw write from TUI
- **THEN** TUI submits raww request through relay operation
- **AND** does not call tmux/ACP transport directly from UI layer

### Requirement: TUI raww error handling taxonomy

TUI raww failure handling SHALL treat canonical relay codes as terminal,
including:
- `validation_unknown_target`
- `validation_cross_bundle_unsupported`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Show deterministic validation error for unsupported target class

- **WHEN** relay returns `validation_invalid_params` for unsupported raww target
  class
- **THEN** TUI shows deterministic raww failure state and does not retry

### Requirement: TUI raww accepted response handling

TUI raww accepted responses SHALL be treated as dispatch-accepted and SHALL NOT
be interpreted as terminal completion.

For ACP accepted responses with
`details.delivery_phase = "accepted_in_progress"`, TUI SHALL preserve the phase
indicator in status presentation where shown.

#### Scenario: Treat accepted_in_progress as non-terminal

- **WHEN** TUI receives raww response with `status = "accepted"`
- **AND** `details.delivery_phase = "accepted_in_progress"`
- **THEN** TUI marks request accepted at dispatch boundary
- **AND** does not mark terminal completion from that response alone
