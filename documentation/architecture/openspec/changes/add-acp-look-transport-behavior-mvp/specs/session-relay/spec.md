## ADDED Requirements

### Requirement: ACP Look Transport Guard

Relay look SHALL reject ACP-backed target sessions in MVP.

When look target resolves to ACP transport, relay SHALL return stable validation
error code `validation_unsupported_transport`.

#### Scenario: Reject look request for ACP target session

- **WHEN** requester invokes relay `look` for a target session backed by ACP
  transport
- **THEN** relay rejects the request with
  `validation_unsupported_transport`

#### Scenario: Preserve existing tmux look behavior

- **WHEN** requester invokes relay `look` for a target session backed by tmux
  transport
- **THEN** relay executes canonical look capture behavior unchanged
