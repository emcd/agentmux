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
   relationship. Inbound auth (peer relay Hello recognized via `[[peers]]`)
   is in scope for Slice 1; outbound connections and permissions masking are
   deferred to a later slice.

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

**D1 — Unified Hello credential: all principals use Hello with `principal_id` + `identity_token`.**
All principal types — session, user, application, and relay — authenticate via
the standard Hello frame. The frame is redesigned to `{schema_version,
principal_id, identity_token}`, replacing the previous `{schema_version,
bundle_name, session_id}`. Both new fields are required `String`s. This is a
breaking change: all existing clients must be updated.

`principal_id` is the claimed identity in `<id>@<namespace>` form:
- Session: `<session_id>@<bundle_name>` (relay extracts bundle name for routing)
- User: `<name>@GLOBAL`
- Application: `<name>@EXTERNAL`
- Peer relay: `<id>@RELAY`

Principal type is resolved at verification time by namespace + token lookup:
- Namespace `@<bundle_name>` + token matches session credential hash → verified
  session principal; relay assigns stable `principal_id` from store.
- Namespace `@<bundle_name>` + token is `"socket-trust"` → session principal,
  unenforced (see D1c); no store entry created; routing uses claimed
  `principal_id`.
- Namespace `@GLOBAL` + token matches user credential hash → user principal.
- Namespace `@EXTERNAL` + token matches application credential hash →
  application principal; relay grants `IdentityIntrospect` rights scoped to
  the associated scope at connection establishment.
- Namespace `@RELAY` + token matches relay credential hash → relay principal;
  authorized for scoped cross-relay message routing.
- Any namespace + unrecognized token (and not `"socket-trust"` on a session
  namespace) → typed error rejection.

Application and relay principals always require a valid, recognized token.

**D1b — Credential provisioning: relay-generated PSK, runtime registration.**
The relay is the sole authority for credential issuance. Credentials are never
configured by path reference in TOML; they are registered at runtime via the
`new peer` command (CLI or MCP `new` meta-tool).

- **Session principals** (`<session_id>@<bundle_name>`): operator calls
  `agentmux new peer <session_id>@<bundle_name>`. Relay generates a 32-byte
  random PSK (see D11), stores the hash in the principal store, and returns the
  raw PSK value plus a config snippet to the caller. The client reads its PSK
  from the well-known path `<state-root>/bundles/<bundle>/sessions/<session>/identity.psk`
  at Hello time. Sessions without a provisioned credential file send the
  `"socket-trust"` constant.
- **User principals** (`<name>@GLOBAL`): same `new peer` flow. For CLI/TUI
  operators not tied to any bundle.
- **Application principals** (`<name>@EXTERNAL`): same `new peer` flow. The
  application stores the PSK at a path of its choosing and presents it in Hello.
- **Relay principals** (`<id>@RELAY`): same `new peer` flow. The peer relay
  stores the PSK at the well-known peer credential path (see D11) and presents
  it in Hello when connecting inbound. Scope for the peer principal is set on
  the principal store record at `new peer` time. Outbound address config
  (`[[peers]]`) is deferred to the outbound routing slice (see D9).
- **PSK rotation** (`agentmux change psk <principal_id>` / MCP `change` meta-tool
  with `command="psk"`): relay generates a new PSK, replaces the hash in the
  principal store, and returns the new PSK. No relay restart required. Slice 1
  rotation affects new connections only — the relay caches the verified
  `principal_id` on the connection context at Hello and does not re-check the
  store on subsequent requests; in-flight connections continue under their
  established `principal_id` until they disconnect. Full revocation dispatch
  (typed error frame + connection teardown for active sessions) lands in Slice 2.
  A future "self-rotation" policy tier may allow principals to rotate their own
  credential without operator round-trip (compromise recovery); deferred.
- **Dynamic session provisioning** (`agentmux new session`) is deferred. The
  required `String` field with `"socket-trust"` leaves this path open without a
  further breaking change.

**D1c — Session credential enforcement: relay-level setting, mandatory on TCP/IP.**
A relay-level `require_session_credentials` setting (default: `false`) controls
enforcement for Unix socket connections. Per-bundle enforcement is not
supported: the relay uses a single socket for all bundles, so a client can
claim any `principal_id` namespace regardless of which bundle they target.
Enforcement must be applied at the relay boundary.

- `false` (default): relay accepts `"socket-trust"` from session connections;
  session gets no `principal_id`. Preserves backward compatibility with all
  existing deployments.
- `true`: `"socket-trust"` and any unrecognized token → typed error rejection.
  Operators opt in relay-wide when all sessions are credentialed.

Configured via `--require-credentials` CLI flag on `agentmux host relay` for
Slice 1; migrates to `relay.toml` in the relay config OpenSpec.

When TCP/IP transport is added, credential enforcement SHALL be mandatory on
that transport path regardless of this setting. The Unix socket option is
scoped to same-host, same-user trust; a network boundary requires
cryptographic proof.

**D2 — Principal store: relay-level file in the state root.**
Location: `<state-root>/identity/principals.json`. Peering and application
trust are relay-wide, not bundle-scoped; a per-bundle store would fragment
@EXTERNAL and @RELAY identity across bundles and require separate registration
per bundle. All four principal types share one relay-level store, keyed by
`principal_id`. The store maps `credential_hash → principal_id + metadata`.
SHA-256 is the required hash algorithm for `credential_hash`: PSKs are 32 bytes
of CSPRNG output, so pre-image resistance is sufficient; password-stretching
(bcrypt/argon2) adds cost without security benefit for this input profile.
File-backed: loaded at startup, written on every mutation (new peer, change
psk, expiry prune). Pruned by expiry. Replace with a heavier store only if
record count demands it.

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
mutual trust via `[[peers]]` configuration (D9).

Cross-relay auth flow (inbound direction, Slice 1):
- Relay A runs `agentmux new peer <relay_b_id>@RELAY`: registers the hash in
  Relay A's relay-level principal store and returns the raw PSK.
- Relay B stores the returned PSK at `<state-root>/peers/<relay_a_alias>.psk`
  (named after the issuing relay — the target Relay B will connect to).
- When Relay B connects to Relay A it sends Hello with
  `principal_id = "<relay_b_id>@RELAY"` and `identity_token = <PSK contents>`;
  Relay A verifies the token against its principal store.

Inbound authentication (peer relay presents a PSK in Hello, verified by the
receiving relay's principal store) is in scope for Slice 1. Outbound
relay-to-relay connections — Relay B initiating a connection to Relay A and
reading the peer credential file — and permissions masking are deferred to a
later slice.

Cross-relay address format (deferred, documented here for routing design
consistency): `<session_id>@<bundle_name>!<relay_id>`, where `<relay_id>` is
the bare id portion of the peer's `[[peers]]` entry (no `@RELAY` suffix — the
`!` encodes the relay-boundary semantics). Example: `claude@myapp!peer-relay`.
This notation follows UUCP bang-path conventions. Display and routing code
should use this canonical form when referencing foreign principals.

**D7 — Principal store is authoritative; no credential secrets in TOML config.**
The relay's principal store is the single source of truth for all registered
credentials. PSK values are never written to TOML configuration files; they are
registered at runtime via `new peer` and stored as hashes only. The `[[peers]]`
config table (D9) carries peer relay scope metadata but no PSK. This eliminates
credential-path-in-config footguns and prevents secrets from appearing in
version-controlled config files. Fail-closed: unrecognized tokens are always
rejected regardless of config state.

**D8 — Principal classes: four namespaces, one store, one connection path.**
The principal store accommodates four principal namespaces, distinguished by a
`principal_type` field. All four authenticate via the standard Hello frame (D1);
the type is resolved by credential partition at verification time.

- **Session** (`<session_id>@<bundle_name>`): an agent session that connected
  via Hello. With a verified credential, the relay issues a stable
  `principal_id`. Host applications map this to their own authorization model.
  Sessions using `"socket-trust"` are not recorded in the store.
- **User** (`<name>@GLOBAL`): a human operator using the CLI or TUI. Not
  bundle-scoped: the relay does not bind a `@GLOBAL` connection to any bundle
  at Hello time, and delivers relevant events from all bundles on the relay to
  that connection. Same authentication path as session principals.
- **Application** (`<name>@EXTERNAL`): a host application (e.g., game engine
  sidecar). Relay grants `IdentityIntrospect` rights at connection time, scoped
  to the associated scope. Credential registered via `new peer <name>@EXTERNAL`.
- **Relay** (`<id>@RELAY`): a peer relay authenticated via Hello. Authorized for
  scoped cross-relay message routing. Credential registered via
  `new peer <id>@RELAY`. Peer relay address config lives in `[[peers]]` (D9).

The unified Hello path eliminates the need for a separate peer handshake
frame. Capability gating (e.g., `IdentityIntrospect` for application principals
only) is enforced at request dispatch time based on the `principal_type`
established at Hello.

**D9 — `[[peers]]` deferred to outbound routing slice.**
The `[[peers]]` TOML config table is not introduced in Slice 1. Inbound peer
relay authentication works entirely via the Hello + principal store path: the
relay recognizes the `@RELAY` namespace in `principal_id`, looks up the hash in
the relay-level principal store, and verifies. Scope for peer relay principals
is stored on the principal store record at `new peer @RELAY` time.

`[[peers]]` lands in the outbound routing slice. At that point it will be
relay-level config (not bundle config), since peering is relay-wide. Expected
fields: `id` (canonical `<id>@RELAY`), `scope`, `address` (outbound endpoint).
No credential storage in TOML — PSKs for outbound connections follow the
well-known peer credential path (D11).

**D10 — `new` and `change` meta-tools; `new.peer` and `change.psk` policy controls.**
Two new MCP meta-tools follow the `lifecycle` meta-tool pattern (single tool,
`command=` dispatch arm):
- `new` tool with `command="peer"`: creates a new principal and generates its
  PSK. Maps to `agentmux new peer <principal_id>` CLI. Optional `--output
  <path>` writes the PSK file directly instead of returning it as output.
- `change` tool with `command="psk"`: rotates the PSK for an existing
  principal. Maps to `agentmux change psk <principal_id>` CLI.

Two new policy controls gate these operations (dot notation per the
`add-do-action-tool` precedent):
- `new.peer`: operator-level; required to create principals.
- `change.psk`: operator-level; required to rotate credentials.

**D11 — PSK format: base64-encoded random bytes, well-known credential file paths.**
PSKs are 32 bytes of CSPRNG output (`OsRng` from the `rand` crate — no
external tooling, cross-platform). Encoding: `base64::engine::general_purpose::STANDARD_NO_PAD`
(no `=` padding; shorter strings, no shell/URL escaping issues if PSKs appear
in those contexts). PSK files contain the raw base64 string; readers strip
trailing whitespace on load. All PSK files and the principal store file must be
created with mode 0600 (owner-read/write only); on Windows, equivalent ACL
enforcement is documented but not enforced by the relay.

Well-known credential file paths:
- **Session principals** (`<session_id>@<bundle_name>`):
  `<state-root>/bundles/<bundle_name>/sessions/<session_id>/identity.psk`
- **Relay principals** (`<id>@RELAY`): peer credential directory at the relay
  state root — `<state-root>/peers/<peer_alias>.psk` — where `<peer_alias>` is
  the portion of the `id` before the `@RELAY` suffix. Stored by the operator
  after `new peer` returns the raw PSK; read by the outbound routing slice when
  Relay B initiates a connection to Relay A.
- **Application principals** (`<name>@EXTERNAL`) and **user principals**
  (`<name>@GLOBAL`): operator-chosen paths; no well-known convention imposed.

The `rand` and `base64` crates are added as dependencies for PSK generation.

## Risks / Trade-offs

- **Rotation window (Slice 1 only)**: `change psk` immediately replaces the
  hash in the principal store. Until Slice 2 lands, active sessions holding the
  old credential are not force-disconnected; they will fail on their next
  request. Document this Slice 1 limitation. Full revocation dispatch (typed
  error frame + connection teardown) lands in Slice 2.
- **Principal store growth**: principals are never pruned without expiry.
  Mitigation: expiry-based pruning at startup or on access.
- **Revocation push gap**: if a stream-event carrier is silently dead, revoked
  events are not delivered. Mitigation: introspection remains authoritative
  (D4); hosts that cache push-state alone violate the spec.

## Open Questions

- **`on_behalf_of` setting mechanism**: the sender attribution field is
  reserved in slice 3 (see Sender Attribution Schema requirement). The exact
  mechanism by which a trusted host supplies or updates this claim for a
  session is deferred. Candidates: an `on_behalf_of` field in the
  `IdentityIntrospect` request, or a separate `RelayRequest::SetSessionClaim`
  variant. Defer until application wiring clarifies the right shape.
- **Dynamic session provisioning scope**: `agentmux new session` is deferred
  but the need is real — game engines and similar embedders will add sessions
  at runtime. The required `String` field with `"socket-trust"` constant leaves
  both provisioning paths open.
- **Cross-relay slice timing**: if the cross-relay use case requires the
  discovery document sooner, slice 4 may need to be pulled forward into a
  follow-on proposal before slices 2–3 are complete.
