## MODIFIED Requirements

### Requirement: Shared ACP Protocol Module

The `src/acp/` module SHALL serve as the shared home for all ACP-specific
implementation: the stdio client (`AcpStdioClient`), the ACP transport
implementation of the `Transport` trait, ACP delivery state, permission state,
and observability. Both the relay delivery subsystem and the `agentmux-acp`
binary SHALL use types from `src/acp/`.

The relay's `acp_client.rs` SHALL be merged into the existing
`src/acp/client.rs`; relay delivery files (`acp_delivery.rs`, `acp_state.rs`,
`permission_state.rs`, `observability.rs`) SHALL move into `src/acp/` as
`transport.rs`, `state.rs`, `permission.rs`, and `observability.rs`
respectively.

#### Scenario: Relay uses shared module for delivery

- **WHEN** the relay delivers messages to an ACP target
- **THEN** it instantiates `AcpTransport` from `src/acp/transport.rs` and
  dispatches through `TransportImpl::Acp`

#### Scenario: Client uses shared module

- **WHEN** the `agentmux-acp` binary connects to an ACP server
- **THEN** it uses `AcpStdioClient` from `src/acp/client.rs`

#### Scenario: No ACP delivery code in relay/delivery/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific delivery files (`acp_delivery.rs`, `acp_state.rs`,
  `permission_state.rs`, `observability.rs`) are present
