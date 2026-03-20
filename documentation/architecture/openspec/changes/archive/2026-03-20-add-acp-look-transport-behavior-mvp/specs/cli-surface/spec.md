## ADDED Requirements

### Requirement: CLI ACP Look Success Surface

For ACP-backed look targets, CLI `agentmux look` SHALL surface relay-authored
successful look payloads and return zero exit status.

CLI SHALL preserve relay `snapshot_lines` ordering and emptiness semantics.

#### Scenario: Surface retained ACP snapshot lines

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to ACP transport with retained snapshot lines
- **THEN** CLI returns successful look payload
- **AND** payload includes `snapshot_lines` ordered oldest -> newest

#### Scenario: Surface empty ACP snapshot as successful look

- **WHEN** operator runs `agentmux look <target-session>` for ACP target with no
  retained snapshot lines
- **THEN** CLI returns successful look payload with `snapshot_lines = []`

#### Scenario: Preserve existing tmux look success path unchanged

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to tmux transport
- **THEN** CLI returns canonical successful look payload
