## MODIFIED Requirements

### Requirement: TUI Transport Failure Semantics

TUI SHALL surface transport/connectivity failures explicitly and SHALL NOT
silently degrade into synthetic success states.

When startup transport is unavailable, TUI SHALL attempt runtime relay
auto-start before rendering an unavailable state.

When TUI auto-spawns a relay during startup, TUI exit SHALL terminate that
auto-spawned relay through the relay's graceful shutdown path. A relay that was
already running at TUI startup SHALL remain untouched on TUI exit.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Attempt relay auto-start on startup transport miss

- **WHEN** operator launches `agentmux tui`
- **AND** relay socket is unavailable at startup
- **THEN** TUI attempts runtime relay auto-start before declaring unavailable

#### Scenario: Stop auto-spawned relay on tui exit

- **WHEN** relay was auto-spawned during TUI startup
- **AND** TUI process exits
- **THEN** TUI issues a graceful relay shutdown for the relay it spawned

#### Scenario: Leave already-running relay on tui exit

- **WHEN** relay was already running at TUI startup
- **AND** TUI process exits
- **THEN** TUI does not issue relay shutdown
