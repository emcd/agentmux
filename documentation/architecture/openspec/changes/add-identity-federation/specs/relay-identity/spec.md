## ADDED Requirements

### Requirement: Verifiable Session Identity

All Hello frames SHALL carry a required `identity_token: String` field. All
principal types (session, application, relay) authenticate via this single
Hello path. The relay SHALL resolve principal type at verification time by
which credential partition the token matches (session `credential_path`
entries, `[[trusted-hosts]]` entries, or `[[trusted-relays]]` entries).

Session credentials SHALL be seeded into the principal store at relay startup
from configured `credential_path` entries. A session presenting a token that
matches a startup-seeded record SHALL be verified on first use without a
prior CLI provisioning step.

Each credential SHALL be bound to its configured principal identity. When the
relay resolves a credential match in any partition, the Hello `session_id`
SHALL match the identity recorded for that credential in configuration. A
credential presented with a mismatched `session_id` SHALL be rejected with a
typed error response before the connection is closed.

A verified session SHALL be assigned a stable `principal_id` distinct from
the ephemeral relay `session_id`. The `principal_id` SHALL persist in the
durable principal store and SHALL remain stable across reconnects by the same
credential.

The relay SHALL recognise the well-known constant `"socket-trust"` as an
intentional unenforced placeholder for session connections. The bundle
configuration SHALL support a `require_session_credentials` setting
(default: `false`) controlling how `"socket-trust"` and unverified tokens are
handled for session connections:
- When `false`: sessions sending `"socket-trust"` SHALL be registered normally
  under socket-level trust without a `principal_id`.
- When `true`: `"socket-trust"` and any unrecognized token SHALL be rejected
  with a typed error response before the connection is closed.

Application and relay principals SHALL always require a recognized token
regardless of the `require_session_credentials` setting.

#### Scenario: Valid session credential establishes authenticated session

- **WHEN** a session client sends a Hello frame with a token matching a
  startup-seeded session credential
- **THEN** the relay verifies the credential against the principal store
- **AND** assigns the session a stable `principal_id`
- **AND** registers the session normally

#### Scenario: socket-trust placeholder connects as unauthenticated when enforcement is off

- **WHEN** a session client sends a Hello frame with `identity_token = "socket-trust"`
- **AND** the bundle has `require_session_credentials = false`
- **THEN** the relay registers the session normally under socket-level trust
- **AND** the session is not assigned a `principal_id`

#### Scenario: socket-trust placeholder rejected when enforcement is enabled

- **WHEN** a session client sends a Hello frame with `identity_token = "socket-trust"`
- **AND** the bundle has `require_session_credentials = true`
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Unrecognized credential is rejected

- **WHEN** a client sends a Hello frame with an `identity_token` not matching
  any credential partition and not equal to `"socket-trust"`
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Credential presented with mismatched session_id is rejected

- **WHEN** a client sends a Hello frame with a recognized `identity_token`
- **AND** the Hello `session_id` does not match the identity configured for
  that credential
- **THEN** the relay sends a typed error response before closing
- **AND** the session is not registered

#### Scenario: Stable principal_id on reconnect

- **WHEN** a client reconnects with the same valid `identity_token`
- **THEN** the relay assigns the same `principal_id` as the previous session
- **AND** the `principal_id` is read from the durable principal store

---

### Requirement: Trusted Host Configuration

The bundle configuration SHALL support a `[[trusted-hosts]]` table that
registers host-application credentials. Each entry SHALL carry:

- `id`: a unique string identifier for the trusted host, used as the
  `session_id` in its Hello frame.
- `credential_path`: a filesystem path to a file containing the host's
  PSK. Inline credential values SHALL NOT be accepted; a configuration
  that supplies an inline credential value SHALL be rejected at load time.
- `scope`: the set of principals the host is permitted to introspect,
  expressed as canonical `session_id@bundle_name` identifiers or bare
  `bundle_name` identifiers (meaning all sessions in that bundle).

A host application authenticates by connecting and sending a Hello frame
with `session_id` equal to its configured `id` and `identity_token` equal
to its PSK. The relay SHALL resolve the token against the `[[trusted-hosts]]`
partition and, on match, SHALL grant `IdentityIntrospect` privilege scoped to
the entry's `scope` at connection establishment. An unmatched credential
SHALL be rejected (fail-closed); there is no default-trust fallback.

The bundle configuration loader SHALL reject any `[[trusted-hosts]]` entry
whose `id` collides with a configured session `session_id` in the same
bundle. Disjoint identity spaces between session and application principals
SHALL be enforced at load time, not at connection time.

#### Scenario: Valid trusted-host credential accepted via Hello

- **WHEN** a host application sends a Hello frame with a token matching a
  `[[trusted-hosts]]` entry
- **THEN** the relay resolves the connection as an application principal
- **AND** grants `IdentityIntrospect` privilege scoped to the entry's `scope`

#### Scenario: Unknown credential rejected

- **WHEN** a host application sends a Hello frame with a token not found in
  any `[[trusted-hosts]]` entry and not matching any other partition
- **THEN** the relay rejects the connection with a typed error response
- **AND** does not grant any trusted-host privilege

#### Scenario: Valid trusted-host credential with mismatched session_id rejected

- **WHEN** a host application sends a Hello frame with a token matching a
  `[[trusted-hosts]]` entry
- **AND** the Hello `session_id` does not equal the entry's configured `id`
- **THEN** the relay rejects the connection with a typed error response
- **AND** does not grant any trusted-host privilege

#### Scenario: Inline credential rejected at config load

- **WHEN** a `[[trusted-hosts]]` entry contains a credential value inline
  (not as a path reference)
- **THEN** the relay refuses to load the configuration
- **AND** emits a validation error identifying the offending entry

#### Scenario: Trusted-host id collision with session id rejected at config load

- **WHEN** a `[[trusted-hosts]]` entry has an `id` that matches a configured
  session `session_id` in the same bundle
- **THEN** the relay refuses to load the configuration
- **AND** emits a validation error identifying the colliding entry

---

### Requirement: Identity Introspection Surface

The relay SHALL expose `RelayRequest::IdentityIntrospect` as a new request
variant. Only connections that have been granted trusted-host privilege
(see: Trusted Host Configuration) SHALL be permitted to issue this request.
A non-trusted connection that issues `IdentityIntrospect` SHALL receive a
typed authorization denial.

An introspection result SHALL include:
- `principal_id`: the stable identity assigned at credential verification.
- `expires_at`: the expiry timestamp for the principal (ISO 8601). Present only
  when the principal has a bounded expiry; absent for principals that never
  expire, rather than carrying a placeholder timestamp.
- `on_behalf_of`: optional opaque host-supplied string carried by the relay.
  This is the same reserved field as in the Sender Attribution Schema: it SHALL
  be included in the response schema but left absent until its setting mechanism
  is specified in a follow-on delta.
- `verified`: boolean indicating the principal passed live verification.

The introspection endpoint is the authoritative source for any
security-gating decision. A host that gates access solely on cached push
events (see: Revocation and Expiry Enforcement) without re-verifying through
introspection violates this requirement.

#### Scenario: Trusted host introspects active session

- **WHEN** a trusted-host connection issues `IdentityIntrospect` for an
  active session within its scope
- **THEN** the relay returns `principal_id`, `expires_at`, and `verified: true`

#### Scenario: Non-trusted connection rejected

- **WHEN** a non-trusted connection issues `IdentityIntrospect`
- **THEN** the relay returns an authorization denial error
- **AND** does not return any identity data

#### Scenario: Introspection of expired principal returns expired result

- **WHEN** a trusted-host introspects a session whose principal has expired
- **THEN** the relay returns the principal record with `verified: false` and
  the recorded `expires_at`

#### Scenario: Introspection of unknown session returns not-found error

- **WHEN** a trusted-host introspects a session_id with no registered principal
- **THEN** the relay returns a typed not-found error

---

### Requirement: Revocation and Expiry Enforcement

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

---

### Requirement: Sender Attribution Schema

Relay Send and Look responses SHALL include an `authenticated_identity` field
when the requesting session has a verified `principal_id`. The field SHALL
carry the stable `principal_id` of the sender, not the ephemeral `session_id`.

If the sender's session carries an `on_behalf_of` claim supplied by a trusted
host, the relay SHALL stamp and carry that claim in the response. The relay
SHALL NOT interpret the `on_behalf_of` value; it is an opaque host-supplied
string.

Sessions without a verified principal SHALL omit the `authenticated_identity`
field rather than populate it with a self-asserted value.

The MCP send and look response schemas SHALL surface `authenticated_identity`
when present. The `on_behalf_of` field is reserved: it SHALL be included in
the response schema as an optional field but its setting mechanism is deferred
to a follow-on spec delta; implementations SHALL leave it absent until that
mechanism is specified.

#### Scenario: Authenticated sender shows principal_id in response

- **WHEN** a Send or Look response is issued for a session with a verified
  principal
- **THEN** the response includes `authenticated_identity` set to the session's
  `principal_id`

#### Scenario: Unauthenticated sender omits attribution field

- **WHEN** a Send or Look response is issued for a session without a verified
  principal
- **THEN** the response does not include `authenticated_identity`

#### Scenario: Authenticated sender's identity carried in delivered envelope

- **WHEN** a Send is dispatched from a session with a verified principal
- **THEN** each UI-stream recipient's `incoming_message` stream event includes
  `authenticated_identity` set to the sender's `principal_id`

#### Scenario: Socket-trust sender omitted from delivered envelope

- **WHEN** a Send is dispatched from a socket-trust session
- **THEN** the `incoming_message` stream event does not include
  `authenticated_identity`
