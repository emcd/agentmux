## MODIFIED Requirements

### Requirement: Revocation and Expiry Enforcement

The relay SHALL apply this requirement when a principal ceases to be valid —
explicit removal, or `expires_at` being reached — and SHALL NOT treat a
credential rotation as a revocation under it: a rotation replaces the credential
while the principal persists and is expected to reconnect with the new one.
Rotation's teardown and event obligations are specified by the
`mcp-tool-surface` capability's MCP Change Tool requirement.

The relay SHALL emit an `identity.snapshot` stream event to a trusted-host
connection at the time the trusted-host stream is established. The snapshot
SHALL carry the current set of active principal records within the trusted
host's scope.

The relay SHALL emit `identity.revoked` events on the existing stream-event
carrier when a principal is revoked. Each event SHALL carry the
`principal_id` and the timestamp of revocation.

When a principal is revoked or expires, the relay SHALL tear down every relay
session bound to that principal. Before closing the connection the relay
SHALL emit a typed error response frame carrying the appropriate error code:
- `runtime_identity_revoked` when the principal was explicitly revoked.
- `runtime_identity_expired` when the principal's `expires_at` was reached.

A bare connection drop without a typed error frame is not permitted; it
would be indistinguishable from `relay_unavailable` at the client.

#### Scenario: Revocation triggers typed error before teardown

- **WHEN** a principal is revoked while a session is active
- **THEN** the relay emits a `runtime_identity_revoked` typed error response
  to the bound session before closing the connection
- **AND** the client can distinguish revocation from a network drop

#### Scenario: Expiry triggers typed error before teardown

- **WHEN** a principal's `expires_at` is reached while a session is active
- **THEN** the relay emits a `runtime_identity_expired` typed error response
  to the bound session before closing
- **AND** the client can distinguish expiry from a network drop

#### Scenario: identity.snapshot delivered on trusted-host stream connect

- **WHEN** a trusted-host stream connection is established
- **THEN** the relay delivers an `identity.snapshot` event carrying the
  current active principal records within the host's scope

#### Scenario: identity.revoked event delivered on revocation

- **WHEN** a principal is revoked
- **THEN** the relay emits an `identity.revoked` event on the stream-event
  carrier to connected trusted-host streams within scope
