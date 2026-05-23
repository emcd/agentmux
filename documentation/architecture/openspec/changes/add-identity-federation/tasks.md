## Prerequisite: `todos/relay/50` — Migrate `request_relay` callers to Hello+envelope protocol

The one-shot `request_relay` path (CLI commands, MCP server, TUI polls) sends
bare `RelayRequest` JSON without a Hello frame, bypassing session registration
and identity verification. This must be resolved before slice 1 can enforce
credentials on all connection paths. See `todos/relay/50` for scope and
implementation details.

## Slice 1 — Authentication Foundation (prerequisite for all other slices)

- [ ] 1.1 Change `HelloFrame` in `src/relay/stream.rs`: `identity_token`
      becomes a required `String` field (was absent; this is breaking — all
      clients must be updated).
- [ ] 1.2 Update all Hello-sending clients (MCP server, TUI, relay client) to
      send `"socket-trust"` as `identity_token` when no credential is
      provisioned.
- [ ] 1.3 Add per-session credential configuration: each session entry in
      bundle config may declare a `credential_path`. Seed the principal store
      from all configured credential paths at relay startup so credentials are
      recognized on first Hello without a prior CLI step.
- [ ] 1.4 Add `[[trusted-hosts]]` TOML config table skeleton to bundle
      configuration schema (`id`, `credential_path`, `scope` as canonical
      `session_id@bundle_name` or bare `bundle_name` identifiers). Seed the
      principal store from these entries at startup (application principal type).
      Inline credential values rejected at load time. Config loader rejects
      any trusted-host `id` that collides with a configured session
      `session_id` in the same bundle.
- [ ] 1.5 Define the principal store schema: `principal_id`, `principal_type`
      (`session` | `application` | `relay`), `credential_hash`, `expires_at`,
      and metadata. Create the store file under `<bundle_runtime>/identity/`
      at relay startup.
- [ ] 1.6 Wire credential verification on Hello handshake: resolve principal
      type by credential partition lookup; assign `principal_id` for verified
      credentials; record `principal_type` for capability gating. Enforce
      credential-to-identity binding: reject with a typed error if the
      recognized credential's configured identity does not match the Hello
      `session_id`. For session connections: accept `"socket-trust"` per
      enforcement policy (D1c). For application connections: always require a
      recognized token.
- [ ] 1.7 Add `require_session_credentials` to bundle configuration schema
      (boolean, default `false`). Thread through to Hello handling.
- [ ] 1.8 Implement expiry-based pruning in the principal store: prune expired
      records on startup and on access.
- [ ] 1.9 Integration test: Hello with valid session credential → session
      registered with stable `principal_id`.
- [ ] 1.10 Integration test: Hello with `"socket-trust"` + enforcement off →
       session registers as socket-trusted (no `principal_id`).
- [ ] 1.11 Integration test: Hello with `"socket-trust"` + enforcement on →
       typed error response, session not registered.
- [ ] 1.12 Integration test: Hello with unrecognized credential → typed error
       regardless of enforcement setting.
- [ ] 1.13 Integration test: reconnect with same credential → same `principal_id`
       returned from store.
- [ ] 1.14 Integration test: application principal Hello (trusted-hosts token)
       → application principal type assigned, IdentityIntrospect right granted.
- [ ] 1.15 Integration test: Hello with valid credential but mismatched
       `session_id` → typed error, session not registered.
- [ ] 1.16 Integration test: trusted-host `id` collision with session
       `session_id` in same bundle → config load fails with validation error.

## Slice 2 — Introspection and Revocation Surface (depends on slice 1)

- [ ] 2.1 Expand `[[trusted-hosts]]` config (bootstrapped in slice 1) with
      full scope validation and inline-credential rejection at load time.
- [ ] 2.2 At Hello verification, when principal type resolves to `application`,
      record scoped `IdentityIntrospect` rights on the connection context for
      use by request dispatch.
- [ ] 2.3 Add `RelayRequest::IdentityIntrospect` variant to
      `src/relay/contract.rs` with fields `target_session: String` and optional
      `bundle_name: Option<String>`.
- [ ] 2.4 Add `RelayResponse::IdentityIntrospect` variant: `principal_id`,
      `expires_at`, `on_behalf_of: Option<String>` (reserved — always `null`
      until setting mechanism is specified), `verified: bool`.
- [ ] 2.5 Gate `IdentityIntrospect` dispatch on `application` principal type;
      return authorization denial for session and unauthenticated connections.
- [ ] 2.6 Implement `identity.snapshot` stream event on trusted-host stream
      connect: deliver current active principal records within the host's scope.
- [ ] 2.7 Implement `identity.revoked` event dispatch on the existing
      stream-event carrier when a principal is revoked.
- [ ] 2.8 Implement relay-side session teardown on revocation/expiry: emit
      typed error response frame (`runtime_identity_revoked` or
      `runtime_identity_expired`) before closing the connection.
- [ ] 2.9 Integration test: application principal introspects active session
      → returns `principal_id`, `expires_at`, `verified: true`.
- [ ] 2.10 Integration test: session principal issues IdentityIntrospect
      → authorization denial, no identity data returned.
- [ ] 2.11 Integration test: principal revoked → session receives
      `runtime_identity_revoked` frame before connection closes.
- [ ] 2.12 Integration test: principal expires → session receives
      `runtime_identity_expired` frame before connection closes.
- [ ] 2.13 Integration test: typed error codes are distinct from
      `relay_unavailable`.

## Slice 3 — Sender Attribution Schema (additive; can overlap with slice 2)

- [ ] 3.1 Add `authenticated_identity: Option<String>` to `RelayResponse::Chat`
      and `RelayResponse::Look` in `src/relay/contract.rs`. Populate from
      session's `principal_id` when verified; omit when unverified.
- [ ] 3.2 Add `on_behalf_of: Option<String>` to the same response variants as
      a reserved field (always `None`/absent until setting mechanism is
      specified in a follow-on delta). Include the field in the schema so
      consumers can handle it without a breaking change when it is activated.
- [ ] 3.3 Update MCP send and look response schemas to surface
      `authenticated_identity` when present; include `on_behalf_of` as a
      reserved optional field.
- [ ] 3.4 Integration test: Chat response for authenticated session includes
      `authenticated_identity` set to `principal_id`.
- [ ] 3.5 Integration test: Chat response for unauthenticated session omits
      `authenticated_identity`.
