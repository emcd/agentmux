## ADDED Requirements

### Requirement: Relay About Validation and Authorization Order

Relay SHALL evaluate `about` requests in this order:

1. request/bundle/session validation
2. authorization policy evaluation
3. response construction

`about` authorization SHALL reuse capability label `list.read`.

If request is valid/resolved but denied by policy, relay SHALL return
`authorization_forbidden` with canonical denial details schema.

#### Scenario: Validate before authorization for unknown session

- **WHEN** request includes unknown `principal_id`
- **THEN** relay returns `validation_unknown_session`
- **AND** does not return `authorization_forbidden` for that request

#### Scenario: Deny valid about request by policy

- **WHEN** request is valid/resolved
- **AND** policy denies `list.read` for requester
- **THEN** relay returns `authorization_forbidden`
