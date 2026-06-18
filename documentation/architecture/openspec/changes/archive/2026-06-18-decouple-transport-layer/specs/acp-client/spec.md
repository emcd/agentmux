## MODIFIED Requirements

### Requirement: Shared ACP Protocol Module

The `src/acp/` module SHALL serve as the shared home for all ACP-specific
implementation: the stdio client (`AcpStdioClient`), the ACP transport
implementation of the `Transport` trait, ACP delivery state, ACP permission
handling, and the ACP worker-driver lifecycle (bootstrap/respawn). Both the relay
delivery subsystem and the `agentmux-acp` binary SHALL use types from `src/acp/`.

The relay's `acp_client.rs` SHALL be merged into the existing `src/acp/client.rs`;
`acp_delivery.rs` and `acp_state.rs` SHALL move into `src/acp/` as `transport.rs`
and `state.rs`; ACP permission handling SHALL be extracted from the inline
handlers in `acp_delivery.rs` into `src/acp/permission.rs`; and the ACP
worker-driver lifecycle SHALL move from `relay/delivery/dispatch/worker.rs` into
`src/acp/worker_driver.rs`. The relay-side `observability.rs` SHALL remain in
`src/relay/delivery/` — it is relay-side pub/sub over relay's own registries, not
ACP protocol code.

#### Scenario: Relay uses shared module for delivery

- **WHEN** the relay delivers messages to an ACP target
- **THEN** it instantiates `AcpTransport` from `src/acp/transport.rs` and
  dispatches through `TransportImpl::Acp`

#### Scenario: Client uses shared module

- **WHEN** the `agentmux-acp` binary connects to an ACP server
- **THEN** it uses `AcpStdioClient` from `src/acp/client.rs`

#### Scenario: No ACP delivery or lifecycle code in relay/delivery/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific delivery, transport, or worker-lifecycle code is
  present (`acp_delivery.rs` and `acp_state.rs` moved to `src/acp/`; the ACP
  bootstrap/respawn driver moved to `src/acp/worker_driver.rs`)
- **AND** `observability.rs` remains, as relay-side pub/sub over relay's own
  registries
