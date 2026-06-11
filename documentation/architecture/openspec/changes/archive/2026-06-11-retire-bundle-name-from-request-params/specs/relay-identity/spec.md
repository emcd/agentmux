## MODIFIED Requirements

### Requirement: Identity Introspection Surface

The relay SHALL expose `RelayRequest::IdentityIntrospect` as a request
variant. Only connections that have been granted trusted-host privilege
(see: Trusted Host Configuration) SHALL be permitted to issue this request.
A non-trusted connection that issues `IdentityIntrospect` SHALL receive a
typed authorization denial.

`target_session` SHALL be supplied as a qualified principal id
(`<id>@<namespace>`). A bare (unqualified) `target_session` — one without a
`@<namespace>` suffix — SHALL be rejected with `validation_invalid_params`
citing `field: "target_session"`. No `bundle_name` qualifier field is
accepted; callers MUST supply a qualified id before issuing the request.

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
  active session within its scope using a qualified principal id
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

#### Scenario: Reject bare target_session in IdentityIntrospect

- **WHEN** a trusted-host connection issues `IdentityIntrospect` with a
  `target_session` that has no `@<namespace>` suffix
- **THEN** relay rejects with `validation_invalid_params`
- **AND** rejection details include `field: "target_session"`
