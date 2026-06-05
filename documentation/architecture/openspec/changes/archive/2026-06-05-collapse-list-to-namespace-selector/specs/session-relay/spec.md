## MODIFIED Requirements

### Requirement: Relay List Authorization

Relay `list_sessions` responses SHALL require policy evaluation for capability
`list.read`.
If requester identity is valid and list access is denied by policy, relay SHALL
return `authorization_forbidden` and SHALL NOT return successful list payload.

The successful list payload collection key SHALL be `principals[]` on the
canonical `ListedBundle` payload (renamed from `sessions[]`); the per-entry
`ListedSession` shape is unchanged.

#### Scenario: Deny list_sessions without successful payload

- **WHEN** requester identity is valid
- **AND** policy denies `list.read` for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return a successful `bundle.principals[]` payload
