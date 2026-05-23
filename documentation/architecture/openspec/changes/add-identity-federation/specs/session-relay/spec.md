## RENAMED Requirements
- FROM: `### Requirement: MVP Trust Boundary`
- TO: `### Requirement: Verified Identity Trust Boundary`

## MODIFIED Requirements

### Requirement: Verified Identity Trust Boundary

The system SHALL enforce a same-host, same-user socket trust boundary as the
access prerequisite. All connecting principals SHALL present an `identity_token`
in the Hello frame (see: `relay-identity` — Verifiable Session Identity).

When a session credential is verified and a `principal_id` is assigned, the
`principal_id` SHALL be the authoritative identity for authorization decisions.
Session connections that send the `"socket-trust"` placeholder operate as
socket-trusted participants with no authenticated principal; in this mode the
relay SHALL fall back to association/socket-driven requester identity, the
same baseline as before identity federation.

Caller-supplied sender-like payload fields SHALL NOT override principal
identity in either mode.

The relay SHALL operate against tmux and relay resources owned by the current
host user. This scope does not change.

#### Scenario: Operate against current user's tmux server

- **WHEN** delivery or reconciliation executes
- **THEN** the system targets tmux resources owned by the current host user

#### Scenario: Verified principal takes precedence over self-asserted session_id

- **WHEN** a session has completed credential verification and holds a
  `principal_id`
- **THEN** relay authorization decisions use the verified `principal_id`
- **AND** self-asserted `session_id` values do not influence principal identity

#### Scenario: Socket-trusted session falls back to requester identity

- **WHEN** a session connects with `identity_token = "socket-trust"`
- **AND** `require_session_credentials = false`
- **THEN** the relay authorizes the session using association/socket-driven
  requester identity
- **AND** the session is not assigned a `principal_id`

#### Scenario: Caller-supplied sender override rejected

- **WHEN** a caller supplies a sender-like payload field that conflicts with
  the established principal or requester identity
- **THEN** the relay authorizes against the established identity
- **AND** does not treat the payload field as authoritative
