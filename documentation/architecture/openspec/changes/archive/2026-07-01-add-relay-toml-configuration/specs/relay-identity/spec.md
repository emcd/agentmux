## MODIFIED Requirements

### Requirement: Verifiable Session Identity

All Hello frames SHALL carry a required `identity_token: String` field. All
principal types (session, application, relay) authenticate via this single Hello
path. The relay SHALL resolve principal type at verification time by which
credential partition the token matches (session `credential_path` entries,
`[[trusted-hosts]]` entries, or `[[trusted-relays]]` entries).

Session credentials SHALL be seeded into the principal store at relay startup
from configured `credential_path` entries. A session presenting a token that
matches a startup-seeded record SHALL be verified on first use without a prior
CLI provisioning step.

Each credential SHALL be bound to its configured principal identity. When the
relay resolves a credential match in any partition, the Hello `session_id` SHALL
match the identity recorded for that credential in configuration. A credential
presented with a mismatched `session_id` SHALL be rejected with a typed error
response before the connection is closed.

A verified session SHALL be assigned a stable `principal_id` distinct from the
ephemeral relay `session_id`. The `principal_id` SHALL persist in the durable
principal store and SHALL remain stable across reconnects by the same
credential.

The relay SHALL recognise the well-known constant `"socket-trust"` as an
intentional unenforced placeholder for session connections. Relay configuration
SHALL support `require-session-credentials` in `<config-root>/relay.toml`
(default: `false`, after override resolution) controlling how `"socket-trust"`
and unverified tokens are handled for session connections:

- When `false`: sessions sending `"socket-trust"` SHALL be registered normally
  under socket-level trust without a `principal_id`.
- When `true`: `"socket-trust"` and any unrecognized token SHALL be rejected
  with a typed error response before the connection is closed.

Application and relay principals SHALL always require a recognized token
regardless of the `require-session-credentials` setting.

#### Scenario: Valid session credential establishes authenticated session

- **WHEN** a session client sends a Hello frame with a token matching a
  startup-seeded session credential
- **THEN** the relay verifies the credential against the principal store
- **AND** assigns the session a stable `principal_id`
- **AND** registers the session normally

#### Scenario: socket-trust placeholder connects as unauthenticated when enforcement is off

- **WHEN** a session client sends a Hello frame with
  `identity_token = "socket-trust"`
- **AND** relay configuration resolves `require-session-credentials = false`
- **THEN** the relay registers the session normally under socket-level trust
- **AND** the session is not assigned a `principal_id`

#### Scenario: socket-trust placeholder rejected when enforcement is enabled

- **WHEN** a session client sends a Hello frame with
  `identity_token = "socket-trust"`
- **AND** relay configuration resolves `require-session-credentials = true`
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Unrecognized credential is rejected

- **WHEN** a client sends a Hello frame with an `identity_token` not matching any
  credential partition and not equal to `"socket-trust"`
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Credential presented with mismatched session_id is rejected

- **WHEN** a client sends a Hello frame with a recognized `identity_token`
- **AND** the Hello `session_id` does not match the identity configured for that
  credential
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Stable principal_id on reconnect

- **WHEN** a client reconnects with the same valid `identity_token`
- **THEN** the relay assigns the same `principal_id` as the previous session
- **AND** the `principal_id` is read from the durable principal store
