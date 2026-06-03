# Change: Add identity federation for trusted host applications

## Why
Agentmux has no authentication layer today. Hello frames carry only
`{schema_version, bundle_name, session_id}` with no credential; session
identity is self-asserted and gated solely by Unix-socket filesystem
permissions. The LitRPG integration exposed this gap: LitRPG needs to key
game-domain authorization mappings to stable Agentmux identity IDs, and
requires the relay to provide a host-application introspection and revocation
surface. A second use case — cross-relay communication between the LitRPG
production relay and the Agentmux R&D relay — requires relay-to-relay trust
as a first-class relationship (scope noted for future slice).

## What Changes
- **BREAKING**: Hello frame extended with a required `identity_token: String`
  field; all existing clients must be updated to send the field. All principal
  types — sessions, host applications, and peer relays — authenticate via the
  standard Hello frame. Clients without a provisioned credential send the
  well-known constant `"socket-trust"`; the relay accepts this for session
  connections when `require_session_credentials = false` (default). Principal
  type (session, application, relay) is resolved at verification time by
  credential partition — no additional Hello field required.
- New bundle-level `require_session_credentials` setting (default: `false`)
  for opt-in session credential enforcement on Unix socket connections. When
  TCP/IP transport is added, credential enforcement SHALL be mandatory on that
  path regardless of this setting.
- New durable principal store per bundle (runtime directory artifact): maps
  verified credentials to stable `principal_id` values that persist across
  relay restarts.
- New `[[trusted-hosts]]` bundle configuration table: `id`, `credential_path`
  (path reference, never inline), and `scope` (bundles/sessions the host may
  introspect).
- New `RelayRequest::IdentityIntrospect` variant: callable only from
  trusted-host connections; returns `principal_id`, `expires_at`, and
  `on_behalf_of` if set.
- Identity snapshot and revocation events (`identity.snapshot`,
  `identity.revoked`) on the existing stream-event carrier.
- Relay-side session teardown on revocation or expiry: relay emits a typed
  error response frame (`runtime_identity_revoked` or
  `runtime_identity_expired`) before closing the connection.
- Additive sender attribution fields (`authenticated_identity`, optional
  `on_behalf_of`) on relay Chat and Look responses; surfaced on MCP send/look
  response schemas.
- Principal store path added to the per-bundle runtime layout.

## Impact
- Affected specs: `relay-identity` (new), `session-relay` (MODIFIED),
  `runtime-bootstrap` (MODIFIED)
- Affected code: `src/relay/stream.rs` (HelloFrame), `src/relay/contract.rs`
  (RelayRequest/RelayResponse), `src/relay/identity.rs` (principal store),
  `src/relay/authorization.rs` (credential verification), bundle configuration
  schema (`[[trusted-hosts]]` table), MCP response schemas
- Scope: slices 1–3 of the 4-slice identity federation plan.
  - Slice 1: auth foundation (Hello credential + principal store)
  - Slice 2: introspection + revocation surface
  - Slice 3: sender attribution schema (additive; early — unblocks
    `embeddable-runtime-api` extension protocol stubs)
  - Slice 4 (discovery document) is out of scope but flagged as a candidate
    for promotion once cross-relay communication use case is scoped.
