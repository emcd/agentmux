# relay-identity Specification

## Purpose
Verifiable session identity for relay `hello` frames plus the trusted-host and identity-introspection surfaces. The spec governs credential-bound `principal_id` assignment (stable across reconnects, stored in the durable principal store at `<state_root>/identity/principals.json`), `socket-trust` placeholder semantics with `require_session_credentials` enforcement toggle, and `[[trusted-hosts]]` configuration with the `IdentityIntrospect` request contract. The sender attribution schema (`authenticated_identity`, `on_behalf_of`) applies to Send/Look responses and `incoming_message` envelopes, with `identity.snapshot` and `identity.revoked` events for revocation and expiry.
## Requirements
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

---

### Requirement: Sender Attribution Schema

Relay Send and Look responses SHALL include an `authenticated_identity` field
when the requesting session has a verified `principal_id`. The field SHALL
carry the stable `principal_id` of the sender, not the ephemeral `session_id`.

If the sender's session carries an `on_behalf_of` claim supplied by an
authenticated intermediary — a trusted host, or a peer relay forwarding on behalf
of an origin principal (see the `cross-relay-routing` capability's Cross-Relay
Sender Attribution Forwarding requirement) — the relay SHALL stamp and carry that
claim in the response. The relay SHALL NOT resolve the `on_behalf_of` value
against its own principal store, SHALL NOT treat it as evidence that the named
principal exists or authored the request, and SHALL NOT use it as an
authorization input. Consumers SHALL read `on_behalf_of` in the context of the
accompanying `authenticated_identity` (the intermediary that asserted it), not as
a globally resolvable principal id.

The relay MAY compose a delivered sender identity from `on_behalf_of` for
display and reply-derivation, provided the composed form also names the
intermediary that asserted it, so a reader cannot mistake the claim for a
locally verified identity. The `cross-relay-routing` capability specifies that
composition for peer-relay forwarding. Composition is not resolution: the
prohibitions above continue to apply to the composed value and to every segment
within it.

Sessions without a verified principal SHALL omit the `authenticated_identity`
field rather than populate it with a self-asserted value.

The MCP send and look response schemas SHALL surface `authenticated_identity`
when present. The `on_behalf_of` field is optional in the response and envelope
schemas. Its setting mechanism for cross-relay forwarding is specified by the
`cross-relay-routing` capability; implementations SHALL leave `on_behalf_of`
absent unless it is set by a specified mechanism (the trusted-host-supplied
`on_behalf_of` on `IdentityIntrospect` records remains a separate, still-reserved
setter).

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

#### Scenario: Peer-relay-forwarded sender carried as on_behalf_of

- **WHEN** a Send is delivered on the receiving relay from a peer relay principal
  that forwarded it on behalf of a verified origin principal
- **THEN** the `incoming_message` envelope includes `authenticated_identity` set
  to the peer relay principal
- **AND** includes `on_behalf_of` set to the origin principal's canonical id
  supplied by the peer, carried without interpretation

#### Scenario: A composed sender identity names its asserting intermediary

- **WHEN** the relay composes a delivered sender identity from an `on_behalf_of`
  claim
- **THEN** the composed identity also names the intermediary that asserted the
  claim
- **AND** the origin segment is not resolved against the local principal store
- **AND** the composed identity is not consulted for any authorization decision

### Requirement: Peer Ingress Scope Grammar

The relay SHALL interpret a relay principal's `scope` as `*` for every namespace
with addressable principals, or a comma-separated set of explicit concrete
namespace names. A single name SHALL be a one-element set. Empty or absent
scope SHALL confer no ingress rights. Namespace sets SHALL cover all
addressable principals in their named namespaces, not individual principals.

The wildcard SHALL include GLOBAL, future bundles, and newly introduced
addressable namespace types immediately, without reconnect, reissuance, a
catalog snapshot, or an extra inclusion switch. Scope SHALL NOT make a
non-addressable namespace or an unsupported operation addressable.

Parsing SHALL trim surrounding whitespace from the input and each set item,
preserve case, and canonicalize sets by sorting and deduplicating names.
Whitespace-only input SHALL mean no rights. Explicit names SHALL follow the
runtime concrete namespace grammar and classification; valid names need not
currently exist. Empty set items and wildcard mixed with names SHALL be invalid.
Persisted peer records SHALL use the same parser as provisioning and updates;
malformed records SHALL fail store loading rather than acquiring default or
implicit rights. Canonical persistence SHALL use `*`, comma-separated names, or
absent scope. The application introspection scope model SHALL remain separate and
unchanged.

#### Scenario: Explicit namespace set

- **WHEN** peer scope is `beta, alpha,beta,GLOBAL`
- **THEN** its canonical scope is `GLOBAL,alpha,beta`
- **AND** it covers all addressable principals in those namespaces only

#### Scenario: Whitespace is trimmed before canonicalization

- **WHEN** peer scope is `  alpha , beta `
- **THEN** its canonical scope is `alpha,beta`

#### Scenario: Wildcard includes present and future addressable namespaces

- **WHEN** a peer holds `scope="*"`
- **THEN** GLOBAL and every current addressable namespace are covered
- **AND** a bundle or newly introduced namespace type becoming addressable is
  covered immediately without changing the credential, grant or connection

#### Scenario: No scope grants nothing

- **WHEN** scope is omitted at registration or is empty/whitespace-only
- **THEN** the principal has no ingress rights

#### Scenario: Validate namespace syntax independently of existence

- **WHEN** a set names a syntactically valid future namespace
- **THEN** registration or update accepts that name without requiring a catalog
  entry
- **AND** malformed set items or wildcard mixed with names fail validation

#### Scenario: Application rights do not expand

- **WHEN** peer namespace scope support is enabled
- **THEN** application introspection and its snapshot/revocation filtering
  retain their own existing scope model

### Requirement: Authoritative Peer Scope Updates

The relay SHALL support in-place replacement and clearing of a registered
peer's scope under the dedicated scope-administration permission. The update
SHALL preserve the PSK and its stored hash, principal identity and type, expiry,
and all unrelated record metadata. It SHALL NOT disconnect the peer or emit
credential revocation events for a scope-only change. PSK rotation SHALL
preserve the latest committed scope rather than widen or restore an older one.

Every ingress delivery and discovery authorization SHALL consult the current
authoritative record for the authenticated peer, not a Hello-time grant
snapshot. A missing or unreadable authority SHALL NOT authorize. Reuse of a
principal id after drop or re-registration SHALL NOT make an obsolete credential
binding current. Request-supplied scope and `on_behalf_of` SHALL NOT authorize.

Scope replacement SHALL be serialized with other principal-store mutations
and final ingress authorization decisions through a store commit. The atomic
rename SHALL be the update linearization point for effective authority;
durable success SHALL additionally require parent-directory sync before
acknowledgement. That sequence SHALL be temporary-file sync, atomic rename
publication, then parent-directory sync. The rename SHALL publish the replacement
as the effective grant while the shared boundary is held; directory-sync failure
SHALL NOT revert effective authority to the old grant. Validation, authorization,
or failure before rename SHALL leave the entire old durable and effective record
unchanged. Parent-directory sync failure SHALL produce an
indeterminate-durability error rather than success, SHALL NOT claim the old
grant remains durable or effective, and SHALL report the effective replacement
scope when scope is returned. An idempotent replacement SHALL succeed only
after completing authorization and directory synchronization; same-scope
repetition SHALL NOT short-circuit synchronization and SHALL follow ordinary
last-writer-wins explicit-request semantics. Concurrent replacements SHALL
take effect in commit order, not as a union.

For Send/Raww the grant decision and admission SHALL be ordered together against
scope commits: no operation SHALL authorize under the old grant, allow a scope
commit to intervene, and then admit under the obsolete decision. Every target
must be covered before any is admitted. A request prepared before the update
but not finally authorized/admitted SHALL use the new grant when ordered after
the commit. Already admitted deliveries SHALL NOT be retroactively cancelled.

For discovery the authorization decision fixing the filtered result SHALL be
ordered against the same scope commits. A result fixed before a commit MAY
arrive after update success; a discovery decision ordered after commit SHALL
use the replacement or a later committed grant. This SHALL apply to both
namespace and concrete-namespace principal discovery on existing connections.

After durable update success, all subsequent authorization decisions SHALL observe
the replacement or a later committed grant. Socket response loss after durable
commit SHALL NOT undo the update or be represented as a pre-commit failure.
An indeterminate-durability error SHALL NOT be represented as success or as a
pre-rename failure. Local scope-update inscriptions SHALL identify the actor,
peer and old/new canonical scope without disclosing credentials.

#### Scenario: Narrow on an existing connection without rotating credentials

- **WHEN** an authorized operator replaces a connected peer's `*` with `alpha`
- **THEN** its connection, PSK, identity, expiry and unrelated metadata persist
- **AND** subsequent Send/Raww and discovery decisions cannot use the old grant

#### Scenario: Update wins a race with delivery preparation

- **WHEN** a request prepares under a grant covering beta
- **AND** a scope update removing beta commits before final authorization/admission
- **THEN** beta is denied and the old preparation does not permit admission

#### Scenario: Admission wins a race with scope update

- **WHEN** a delivery is authorized and admitted before a narrowing commits
- **THEN** it may execute after update success
- **AND** the scope update does not retroactively cancel it

#### Scenario: Discovery decisions straddle a scope update

- **WHEN** one discovery result is fixed before a narrowing commit and another
  decision is ordered after that commit on the same peer connection
- **THEN** the first may still arrive after the update response
- **AND** the second contains only data covered by the replacement grant

#### Scenario: Failed update before publication leaves the old record intact

- **WHEN** validation, authorization or persistence of a scope update fails
  before rename publication
- **THEN** the prior scope, PSK, identity, expiry and metadata remain unchanged
- **AND** subsequent ingress decisions continue to use the old scope

#### Scenario: Directory-sync failure is indeterminate, not old-grant-intact

- **WHEN** rename publication succeeds but parent-directory sync fails
- **THEN** the replacement remains the effective grant
- **AND** the caller receives an indeterminate-durability error reporting the
  effective replacement scope, not success and not an old-grant-intact claim

#### Scenario: Concurrent rotation does not restore stale scope

- **WHEN** rotation and scope replacement run concurrently
- **THEN** they serialize without overwriting each other's fields
- **AND** the resulting record has the rotated credential and replacement scope
  if both commit successfully

#### Scenario: Clear and retry safely

- **WHEN** an administrator successfully sets empty scope and repeats it
- **THEN** both requests succeed without rotating or disconnecting the peer
- **AND** the repetition completes authorization and directory synchronization
  rather than short-circuiting as a no-op
- **AND** the peer has no delivery or discovery rights

#### Scenario: Missing peer authority cannot reuse an old grant

- **WHEN** a peer's record is dropped or its credential binding is superseded
- **THEN** a not-yet-admitted ingress request cannot authorize from a cached
  grant or a newly registered record sharing only its principal id
