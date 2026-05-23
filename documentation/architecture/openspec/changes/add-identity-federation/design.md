## Context

Agentmux relay today has no authentication layer. Hello frames are
`{schema_version, bundle_name, session_id}` with identity self-asserted;
session registration uses first-claimer-wins dedup on `(bundle_name,
session_id)` with no credential verification. This proposal introduces an
auth layer from scratch — it does not extend one. That is the single
largest cost item and gates slices 2 and 3.

Two use cases drive the design:
1. **Host-application trust**: A game engine under development embeds or
   sidecars Agentmux and must key domain authorization to stable Agentmux
   identity IDs. It needs a revocation-aware introspection surface.
2. **Cross-relay communication**: An Infrastructure group operates a relay
   separate from the R&D relay; both relays need to exchange messages across
   relay boundaries. This makes relay-to-relay trust a first-class
   relationship (noted here; scoped in a future slice).

## Goals / Non-Goals

Goals:
- Stable `principal_id` values distinct from ephemeral relay `session_id`s.
- Durable principal store that survives relay restarts.
- Host-app introspection surface callable only by trusted-host connections.
- Revocation with typed error frames before teardown (not silent drops).
- Additive sender attribution on relay responses (unblocks extension protocol).

Non-Goals (this proposal):
- MCP introspection tool: an `introspect(any-identity)` MCP tool would let
  any agent session act as a trusted-host equivalent, collapsing the
  trusted-host vs. arbitrary-client distinction. At most a narrow
  `command=identity` arm (caller's own principal only) may be considered
  later; not on the host-app motivating path and deferred.
- Signed JWT assertions: valuable for external multi-host deployments where
  round-trip to the introspection endpoint is a cost. Not MVP — requires a
  crypto + key-rotation subsystem the project has never had, and makes
  revocation hard (valid until TTL regardless of revoke event). Revisit only
  when a concrete external deployment shows the round-trip is a problem.
- mTLS / certificate-based auth: defer until cross-host network deployment.
- Discovery document (slice 4): the cross-relay use case may promote this;
  out of scope here.

## Decisions

**D1 — Unified Hello credential: all principals use Hello with required `identity_token`.**
All principal types — session, application, and relay — authenticate via the
standard Hello frame. The `identity_token` field is a required `String` (not
optional). This is a breaking change to the Hello protocol: all existing
clients must be updated to send the field.

Principal type is resolved at verification time by which credential partition
the token matches:
- Token matches a session `credential_path` entry → session principal.
- Token matches a `[[trusted-hosts]]` entry → application principal; relay
  grants `IdentityIntrospect` rights scoped to the entry's `scope` at
  connection establishment.
- Token matches a `[[trusted-relays]]` entry → relay principal (future slice).
- Token is `"socket-trust"` → session principal, unenforced (see D1c).
- Token unrecognized and not `"socket-trust"` → typed error rejection.

Application and relay principals always require a valid, recognized token.

**D1b — Credential provisioning by principal type.**
- **Session principals**: configured `credential_path` per session entry in
  bundle config, loaded at relay startup and used to seed principal store
  records. Sessions without a provisioned credential send the well-known
  constant `"socket-trust"`. The relay recognizes this constant as an
  intentional unenforced placeholder — not as an opaque token to verify.
- **Application and relay principals**: CLI-generated PSK via `agentmux new
  peer` (or `agentmux new friend`). Credential values stored in files and
  referenced by path in config — never written inline in TOML. Rotation:
  regenerate via CLI, distribute to both sides, and restart.
- **Dynamic session provisioning** (`agentmux new session`) is deferred.
  Needed for runtime session creation (e.g., AI players added from within a
  game interface). The required `String` field with the `"socket-trust"`
  constant leaves this path open without a further breaking change.

**D1c — Session credential enforcement: configurable per bundle, mandatory on TCP/IP.**
A bundle-level `require_session_credentials` setting (default: `false`)
controls enforcement for Unix socket connections:
- `false` (default): relay accepts `"socket-trust"` from session connections;
  session gets no `principal_id`. Preserves backward compatibility with all
  existing bundle entries.
- `true`: `"socket-trust"` and any unrecognized token → typed error rejection.
  Operators opt in per bundle when all sessions are credentialed.

When TCP/IP transport is added, credential enforcement SHALL be mandatory on
that transport path regardless of this setting. The Unix socket option is
scoped to same-host, same-user trust; a network boundary requires
cryptographic proof.

**D2 — Principal store: per-bundle file in the bundle runtime directory.**
Location: `<bundle_runtime>/identity/principals.db` (format TBD at
implementation; a simple JSON or sqlite file is sufficient for MVP). The store
maps `credential_hash → principal_id + metadata`. Pruned by expiry. Replace
with a heavier store only if record count demands it.

Reference model: the game engine under development maintains a state directory
with a JSON file containing one auth token per session ID — a close analog.
Peer relays and host applications are clearly principals. Whether agent
sessions share the same principal store or use a separate credential index is
an open question (see Open Questions).

**D3 — IdentityProvider interface: embedded vs. external.**
Introduce an `IdentityProvider` trait with two implementations:
- `EmbeddedIdentityProvider`: direct in-process call; used when relay runs
  embedded in the host process. Trust boundary is the process; no socket;
  signing is near-pointless.
- `ExternalIdentityProvider`: signed assertions + live introspection endpoint;
  used for external/sidecar deployments. Not in scope for this proposal but
  the interface must be designed for it.
The shared, topology-invariant contract is the **introspection result schema**,
not the transport.

**D4 — Authority model: introspection is authoritative; push is optimization.**
`identity.snapshot` on trusted-host stream connect + `identity.revoked` events
provide low-latency awareness. A security-gating decision MUST be re-derivable
synchronously via `RelayRequest::IdentityIntrospect`. A host that caches only
push-state leaves a revocation-to-enforcement window (a silently-dead stream
drops events; the cached state looks valid). This constraint must appear in
the spec, not just here.

**D5 — Typed error frame before teardown.**
When the relay terminates a session due to revocation or expiry it MUST emit
a typed error response frame (`runtime_identity_revoked` or
`runtime_identity_expired`) before closing the connection. A bare drop maps
via `map_relay_request_failure` to `relay_unavailable`, indistinguishable from
a network failure. This is the same surface class fixed in the relay/41
changeset.

**D6 — One-relay = one-identity-authority.**
Within a single relay deployment, one relay is the sole identity authority.
Cross-relay identity verification requires two separate authorities to establish
mutual trust — not a shared store. A `[[trusted-relays]]` peer config
(analogous to `[[trusted-hosts]]`) is the candidate mechanism; out of scope
for this proposal.

**D8 — Principal classes: three types, one store, one connection path.**
The principal store accommodates three principal types, distinguished by a
`principal_type` field. All three authenticate via the standard Hello frame
(D1); the type is resolved by credential partition at verification time.

- **Session** (`session`): an agent session that connected via Hello. With a
  verified credential, the relay issues a stable `principal_id`. Host
  applications map this to their own authorization model (e.g., game roles,
  code review permissions). Sessions using `"socket-trust"` are not recorded
  in the store.
- **Application** (`application`): a host application that authenticated via
  a `[[trusted-hosts]]` credential in Hello. Relay grants `IdentityIntrospect`
  rights at connection time, scoped to the entry's `scope`. In the code review
  example, the application is the sender/recipient principal for review
  requests; session `principal_id` values appear in attribution fields.
- **Relay** (`relay`): a peer relay that authenticated via a
  `[[trusted-relays]]` credential in Hello. Authorized for cross-relay message
  routing. Out of scope for slice 1 but the store schema must accommodate it.

The unified Hello path eliminates the need for a separate peer handshake
frame. Capability gating (e.g., `IdentityIntrospect` for application
principals only) is enforced at request dispatch time based on the
`principal_type` established at Hello.

**D7 — Trusted-host config: path reference, never inline secret.**
`[[trusted-hosts]]` entries carry `credential_path` (a filesystem path to a
file containing the secret). This is the first secret in any Agentmux config
file. A deliberate decision: start with pre-shared secret by path; defer
key-management infrastructure. Unknown/unmatched credentials are rejected
(fail-closed); there is no default-trust fallback.

## Risks / Trade-offs

- **PSK rotation gap**: rotating a credential file while the relay is running
  means in-flight sessions hold the old token. Mitigation: relay restart
  invalidates in-memory state; reconnecting clients re-authenticate with the
  new token. Document this constraint explicitly.
- **Principal store growth**: principals are never pruned without expiry.
  Mitigation: expiry-based pruning at startup or on access.
- **Revocation push gap**: if a stream-event carrier is silently dead, revoked
  events are not delivered. Mitigation: introspection remains authoritative
  (D4); hosts that cache push-state alone violate the spec.

## Open Questions

- **Exact PSK format**: raw binary file vs. base64-encoded string vs. TOML
  path-ref pointing to a PEM file. TBD at implementation; spec states "path
  reference to credential file" without prescribing file format.
- **`on_behalf_of` setting mechanism**: the sender attribution field is
  reserved in slice 3 (see Sender Attribution Schema requirement). The exact
  mechanism by which a trusted host supplies or updates this claim for a
  session is deferred. Candidates: an `on_behalf_of` field in the
  `IdentityIntrospect` request, or a separate `RelayRequest::SetSessionClaim`
  variant. Defer until application wiring clarifies the right shape.
- **Dynamic session provisioning scope**: `agentmux new session` is deferred
  but the need is real — game engines and similar embedders will add sessions
  at runtime (e.g., AI players joining from within game interfaces). When
  scoped, decide whether provisioning issues a credential file that the
  session presents at Hello, or whether the relay auto-registers dynamically
  created sessions via an internal API. The required `String` field with
  `"socket-trust"` constant leaves both paths open.
- **Cross-relay slice timing**: if the cross-relay use case requires the
  discovery document sooner, slice 4 may need to be pulled forward into a
  follow-on proposal before slices 2-3 are complete.
