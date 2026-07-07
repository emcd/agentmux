## MODIFIED Requirements

### Requirement: TUI Auto-Spawn Relay Lifecycle Ownership

Relay auto-start from `agentmux tui` SHALL establish TUI ownership of the relay
it spawns.

`agentmux tui` SHALL terminate a relay process on TUI exit if and only if that
relay was auto-spawned by that same TUI invocation. A relay that was already
running when the TUI started (service manager or operator action) SHALL NOT be
terminated on TUI exit.

The auto-spawned relay SHALL be started in its own process group so its
lifecycle is governed solely by the TUI's explicit termination signal, not by
incidental terminal signal propagation.

Termination SHALL reuse the relay's standard signal-driven shutdown path, which
prunes the tmux sessions the relay owns and reaps the tmux server when it becomes
unowned. A TUI-auto-spawned relay is therefore an ad hoc single-operator
convenience, not a durable shared relay.

#### Scenario: Stop auto-spawned relay on tui exit

- **WHEN** `agentmux tui` auto-spawns a relay because none was reachable
- **AND** the TUI later exits normally or via signal
- **THEN** the TUI sends the spawned relay a graceful termination signal
- **AND** relay shutdown prunes the tmux sessions it owns and reaps the tmux
  server when it becomes unowned

#### Scenario: Leave already-running relay after tui exit

- **WHEN** a relay is already reachable when `agentmux tui` starts
- **AND** the TUI does not auto-spawn a relay
- **AND** the TUI later exits
- **THEN** the TUI does not signal or terminate that relay
- **AND** the relay remains running under its existing lifecycle controls
  (`agentmux host relay`, service manager, or operator action)
