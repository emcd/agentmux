## MODIFIED Requirements

### Requirement: TUI raww error handling taxonomy

TUI raww failure handling SHALL treat canonical relay codes as terminal,
including:

- `validation_unknown_target`
- `validation_unsupported_operation`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Show deterministic validation error for unsupported target class

- **WHEN** relay returns `validation_unsupported_operation` for an unsupported
  raww target class
- **THEN** TUI surfaces the error as terminal without retry

### Requirement: TUI Transport Failure Semantics

TUI SHALL surface transport/connectivity failures explicitly and SHALL NOT
silently degrade into synthetic success states.

When startup transport is unavailable, TUI SHALL attempt runtime relay
auto-start before rendering an unavailable state.

Auto-started relay lifecycle remains external; TUI exit SHALL NOT auto-stop
relay.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Attempt relay auto-start on startup transport miss

- **WHEN** operator launches `agentmux tui`
- **AND** relay socket is unavailable at startup
- **THEN** TUI attempts runtime relay auto-start before declaring unavailable

#### Scenario: Do not auto-stop relay on tui exit

- **WHEN** relay was auto-started during TUI startup
- **AND** TUI process exits
- **THEN** TUI does not issue relay shutdown solely due to TUI exit
