## ADDED Requirements

### Requirement: Persistent Relay Client Mode for MCP and TUI

MCP and TUI relay clients SHALL use persistent relay stream connections in MVP
rather than per-request reconnect behavior.

MCP and TUI clients SHALL perform `hello` registration on stream setup before
sending relay request frames.

`hello` registration in runtime clients SHALL use canonical routing identity:

- associated runtime `bundle_name`
- canonical `session_id`
- `client_class`

#### Scenario: MCP establishes persistent agent-class relay stream

- **WHEN** MCP performs first relay-backed operation in runtime
- **THEN** MCP establishes persistent relay stream
- **AND** registers with `hello` using associated `bundle_name`,
  canonical `session_id`, and `client_class=agent`

#### Scenario: TUI establishes persistent ui-class relay stream

- **WHEN** TUI starts and relay connectivity is available
- **THEN** TUI establishes persistent relay stream
- **AND** registers with `hello` using associated `bundle_name`,
  canonical `session_id`, and `client_class=ui`

### Requirement: Stream Reconnect Behavior

On stream disconnect, clients SHALL attempt reconnect with same identity and
repeat `hello` registration.

Reconnect failures SHALL be surfaced as `relay_unavailable` errors in existing
caller-facing paths.

#### Scenario: Re-register identity after reconnect

- **WHEN** client stream reconnect succeeds after disconnect
- **THEN** client sends `hello` with same identity
- **AND** relay binds latest stream to that identity

#### Scenario: Surface relay unavailable on reconnect failure

- **WHEN** reconnect attempt fails to establish stream
- **THEN** client surfaces `relay_unavailable` in caller-facing error path
