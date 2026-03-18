## ADDED Requirements

### Requirement: CLI ACP Look Rejection Passthrough

For ACP-backed look targets, CLI `agentmux look` SHALL surface relay-authored
`validation_unsupported_transport` and return non-zero exit status.

CLI SHALL NOT remap this rejection to alternate look error codes.

#### Scenario: Surface unsupported-transport for ACP look target

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to ACP transport
- **THEN** CLI surfaces `validation_unsupported_transport`
- **AND** exits with non-zero status

#### Scenario: Preserve existing tmux look success path

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to tmux transport
- **THEN** CLI returns canonical successful look payload
