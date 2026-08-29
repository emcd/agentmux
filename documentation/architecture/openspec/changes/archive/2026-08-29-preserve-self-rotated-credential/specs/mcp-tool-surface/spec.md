## MODIFIED Requirements

### Requirement: MCP Change Tool

The system SHALL expose a meta-tool `change` that rotates an existing
principal's PSK. `change` SHALL require `command="psk"`.

`change psk` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

The relay SHALL generate a new PSK for the existing principal and apply the same
credential-destination selector as `new peer` (Response by default, Path for
`output_path`, Config for `write_to_config`), with identical session-only Config
derivation, path preconditions, and safe-segment validation. The relay SHALL
stage and commit the destination before revoking the live connections that hold
the prior credential, other than the requesting connection; a rejected or failed
destination SHALL NOT rotate the PSK or revoke any connection. `change` is a
relay-wide operation: the relay SHALL authorize the connection principal against
an `all`-scoped `change.psk` grant, and a bundle-relative `home` grant SHALL be
insufficient.

Rotation replaces a credential while the principal persists, so it is not a
revocation under the `relay-identity` capability's Revocation and Expiry
Enforcement requirement, and carries its own teardown obligations.

The relay SHALL tear down every live session authenticated with the prior
credential, other than the requesting connection. Before closing each such
session the relay SHALL emit a `runtime_identity_revoked` typed error response
frame; a bare connection drop without a typed error frame is not permitted, as it
would be indistinguishable from `relay_unavailable` at the client.

The relay SHALL emit an `identity.revoked` event on the existing stream-event
carrier to every connected trusted-host stream whose scope covers the rotated
principal. Each event SHALL carry the `principal_id` and the timestamp of
revocation.

The relay SHALL NOT tear down the requesting connection when a principal rotates
its own credential. That connection is the caller awaiting the response, which
carries the only copy of the new PSK when the Response destination was selected;
tearing it down discards that response and leaves the principal holding no
credential matching the hash the relay has already committed. The relay SHALL
still emit the `identity.revoked` event for a self-rotation, because a trusted
host holding a cached view of that credential must drop it regardless of who
initiated the change. Excluding the requester cannot leave another session alive
holding the prior credential, because the stream registry admits at most one live
connection per `principal_id`.

#### Scenario: Advertise change tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `change`

#### Scenario: Rotate PSK for an existing principal

- **WHEN** a caller invokes `change` with `command="psk"` and a `principal_id`
- **THEN** the relay rotates the principal's PSK and returns the new value
- **AND** omits the raw PSK from the response when a file destination was
  selected

#### Scenario: Rejected destination leaves the credential unrotated

- **WHEN** a `change psk` request selects a destination the relay rejects
- **THEN** the relay returns the corresponding validation error
- **AND** does not rotate the PSK or revoke any live connection

#### Scenario: Self-rotation returns the rotated credential

- **WHEN** a principal whose connection authenticated with its own credential
  rotates its own PSK with the Response destination
- **THEN** the relay returns the rotated PSK on that connection
- **AND** does not tear that connection down

#### Scenario: Rotation revokes another principal's live session

- **WHEN** a caller rotates the PSK of a different principal that holds a live
  authenticated session
- **THEN** that session receives a `runtime_identity_revoked` typed error frame
  before its connection is closed

#### Scenario: Rotation fans out identity.revoked to trusted hosts

- **WHEN** a caller rotates a principal's PSK
- **THEN** the relay emits an `identity.revoked` event on the stream-event
  carrier to each connected trusted-host stream whose scope covers that
  principal
- **AND** the event carries the `principal_id` and the timestamp of revocation

#### Scenario: Self-rotation still notifies trusted hosts

- **WHEN** a principal rotates its own PSK
- **THEN** trusted-host streams within scope still receive the
  `identity.revoked` event
- **AND** the requesting connection is not torn down
