## MODIFIED Requirements

### Requirement: Relay List Bundle Live-State Payload

Successful relay `list_sessions` responses SHALL include:

- `schema_version`
- `bundle` object:
  - `id`
  - `state` (`up`|`down`)
  - `state_reason_code` (required when `state=down`)
  - `state_reason` (optional)
  - `sessions` (array)

Each `sessions` entry SHALL include:

- `id`
- `name` (optional)
- `transport` (`tmux`|`acp`)

`sessions` SHALL represent configured bundle members (not live members only).

MVP list down-state reason mapping SHALL be deterministic:

- `not_started`: expected bundle relay socket path absent
- `relay_unavailable`: socket exists but connect/request probe fails

#### Scenario: Return canonical list_sessions payload

- **WHEN** relay processes a valid single-bundle `list_sessions` request
- **THEN** relay returns successful payload with `bundle.id`, `bundle.state`,
  and `bundle.sessions[]`

#### Scenario: Report not_started when relay socket is absent

- **WHEN** list state derivation observes missing expected relay socket path
- **THEN** relay/adapter state contract uses `state=down`
- **AND** `state_reason_code=not_started`

#### Scenario: Report relay_unavailable when socket exists but probing fails

- **WHEN** relay socket path exists
- **AND** connection or request probe fails
- **THEN** state contract uses `state=down`
- **AND** `state_reason_code=relay_unavailable`

### Requirement: Relay List Authorization

Relay `list_sessions` responses SHALL require policy evaluation for capability
`list.read`.
If requester identity is valid and list access is denied by policy, relay SHALL
return `authorization_forbidden` and SHALL NOT return successful list payload.

#### Scenario: Deny list_sessions without successful payload

- **WHEN** requester identity is valid
- **AND** policy denies `list.read` for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return a successful `bundle.sessions[]` payload

## ADDED Requirements

### Requirement: Relay List Sessions Request Scope

Relay SHALL support only single-bundle session listing requests in MVP.
Relay SHALL NOT accept all-bundle list selectors.

#### Scenario: Reject all-bundle relay list selector

- **WHEN** a caller requests relay list with all-bundle selector semantics
- **THEN** relay rejects request with `validation_invalid_params`
