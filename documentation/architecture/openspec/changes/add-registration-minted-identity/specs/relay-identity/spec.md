## ADDED Requirements

### Requirement: Registration-Minted Identity Branch

Identity minted for dispatch input SHALL carry its minting provenance:
credential-minted (verification or authorized provisioning against a
store credential, bound to the credential and principal-store epoch)
or registration-minted (transport admission by authorized
administrative mutation, bound to the runtime instance plus the
connection and registration generation epoch). Minting SHALL occur
only through one of these two rules; no other producer SHALL mint
dispatch input.

Registration minting SHALL apply only to Session and User principals
admitted with `require-session-credentials` resolved off. Application
and Relay principals SHALL always require a recognized token and
SHALL NOT receive a credential-free branch under any setting.
Registration minting SHALL create no store credential and SHALL cause
no other store side effect.

Registration-minted identity SHALL be labeled non-store-authenticated
in every local record, diagnostic, and attribution: this is local
minting and runtime provenance within the owning relay, expressed
through the existing omission of `authenticated_identity` for the
local unverified requester. Admitted claims SHALL never render as
authenticated, and a registration-minted identity SHALL never
upgrade to credential-minted standing. A fresh explicit
registration SHALL be a separate mint, never an in-place provenance
upgrade or downgrade. Cross-relay attribution is unchanged: a
peer-supplied `on_behalf_of` value stays advisory and
`authenticated_identity` stays the independently credential-backed
peer, with no new wire field conveying the distinction.

A registration-minted identity SHALL be invalidated by live-claim
replacement, reconnect with a raised generation, connection
disconnect, runtime shutdown, and bound-configuration change, with no
lingering use and no grace reuse. Home namespace SHALL be frozen from
the admitted registration and bound configuration, never from wire
per-request namespace. Per-operation authorization, current authority
at final admission, and the single handler surface SHALL apply
identically on both branches.

#### Scenario: Credential-free session receives the registration branch

- **WHEN** a session client sends a Hello frame with
  `identity_token = "socket-trust"`
- **AND** relay configuration resolves `require-session-credentials = false`
- **THEN** the relay mints registration-minted dispatch input bound to
  the runtime instance and connection generation
- **AND** records it as non-store-authenticated
- **AND** creates no store credential

#### Scenario: Application and relay kinds have no credential-free branch

- **WHEN** an application or relay client sends a Hello frame with
  `identity_token = "socket-trust"`
- **THEN** the relay rejects the Hello with a typed error response
  regardless of the `require-session-credentials` setting

#### Scenario: Replacement and disconnect invalidate the registration branch

- **WHEN** a live conflicting claim replaces the registration, the
  connection disconnects, or the runtime shuts down
- **THEN** further dispatch with that registration-minted input is
  refused

#### Scenario: Registration branch authorizes per operation without elevation

- **WHEN** a registration-minted identity dispatches an operation
- **THEN** authorization runs per operation in the requester home
  namespace under current grants
- **AND** responses and attribution never present the holder as
  store-authenticated

#### Scenario: Fresh registration is a separate mint

- **WHEN** a principal re-registers after its registration-minted
  input is invalidated
- **THEN** the new registration mints a new input bound to the new
  generation
- **AND** the prior input is not revived or converted
