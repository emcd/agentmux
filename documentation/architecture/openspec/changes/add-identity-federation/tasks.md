## Pre-implementation decisions (resolved)

- [x] P.1 PSK format: base64-encoded random bytes in `identity.psk` file.
      Relay generates via `OsRng` (32 bytes); reader strips trailing whitespace.
      STANDARD_NO_PAD encoding. SHA-256 for credential_hash. `rand` and `base64`
      crates added as dependencies. Documented in D11.
- [x] P.2 `agentmux new peer` / `agentmux change psk` commands added as Slice 1
      tasks (1.5 and 1.6). Relay generates PSK at runtime; no external tooling.
      Principal-id (not path) is the primary argument. Documented in D1b, D10.

## Slice 1 — Authentication Foundation (prerequisite for all other slices)

- [ ] 1.1 Redesign `HelloFrame` in `src/relay/stream.rs`: replace `bundle_name`
      and `session_id` with `principal_id: String` (claimed identity in
      `<id>@<namespace>` form) and add `identity_token: String`. Both fields are
      required. This is a breaking change — all clients must be updated.
- [ ] 1.2 Update all Hello-sending clients (MCP server, TUI, relay client) to
      send `principal_id = "<session_id>@<bundle_name>"` and read `identity.psk`
      from the well-known path
      `<state-root>/bundles/<bundle>/sessions/<session>/identity.psk`, sending
      its contents as `identity_token`. Send `"socket-trust"` as `identity_token`
      when the file is absent or unreadable.
- [ ] 1.3 Define the well-known credential file path conventions. Add helpers in
      `src/runtime/paths.rs` for:
      (a) session PSK path: `<state-root>/bundles/<bundle>/sessions/<session>/identity.psk`
          (derived from bundle name and session id).
      (b) peer PSK path: `<state-root>/peers/<peer_alias>.psk`
          (derived from state root and peer alias — the `id` portion before the
          `@RELAY` suffix). Used by the outbound routing slice; helpers defined
          here so path conventions are consistent.
      (c) principal store path: `<state-root>/identity/principals.json`.
      All three paths must be created with mode 0600 (owner-read/write only).
      Document conventions in a README note under the identity subsystem.
      Note: operators must register at least one credential before setting
      `require_session_credentials = true`; document the bootstrap sequence to
      prevent lockout.
- [ ] 1.4 Add `rand` (with `getrandom` backend) and `base64` to `Cargo.toml`.
      Implement a crate-internal `generate_psk() -> String` helper that produces
      a 32-byte CSPRNG output encoded as `STANDARD_NO_PAD` base64.
- [ ] 1.5 Implement `agentmux new peer <principal_id>` CLI command and the `new`
      MCP meta-tool (`command="peer"`). Relay: call `generate_psk`, hash with
      SHA-256, store in principal store (see 1.8), return raw PSK + config snippet
      to caller. Optional `--output <path>` flag writes the PSK to the specified
      path instead of returning it; `--output` paths must be absolute, the relay
      refuses to follow symlinks during write, and parent directories must already
      exist (no auto-creation). Supported namespaces: `@<bundle>`, `@GLOBAL`,
      `@EXTERNAL`, `@RELAY`. For `@RELAY` principals, `scope` is set on the
      principal store record at registration time.
- [ ] 1.6 Implement `agentmux change psk <principal_id>` CLI command and the
      `change` MCP meta-tool (`command="psk"`). Relay: generate new PSK, replace
      hash in principal store, return new PSK to caller. Slice 1: store update
      only; revocation dispatch to active sessions lands in Slice 2.
- [ ] 1.7 Add `new.peer` and `change.psk` to `PolicyControls` in
      `src/relay/authorization.rs` (dot-notation fields, operator-level defaults
      following `add-do-action-tool` precedent). Update
      `data/configuration/policies.toml` and
      `.auxiliary/configuration/agentmux/policies.toml` operator policy to
      include both controls.
- [ ] 1.8 Define the principal store schema: `principal_id`, `principal_type`
      (`session` | `user` | `application` | `relay`), `credential_hash`
      (SHA-256 hex), `scope` (optional; set for `@RELAY` and `@EXTERNAL`
      principals at registration), `expires_at`, and metadata. Create and load
      `<state-root>/identity/principals.json` at relay startup. Write with mode
      0600 on every mutation (new peer, change psk, expiry prune).
- [ ] 1.9 Wire credential verification on Hello handshake: parse `principal_id`
      namespace to determine principal type; SHA-256-hash `identity_token` and
      look up in relay-level principal store; use constant-time comparison for
      the hash lookup (e.g. `subtle::ConstantTimeEq`) to avoid timing leaks;
      assign verified `principal_id` and record `principal_type` for capability
      gating. Enforce credential-to-identity binding: reject with a typed error
      if the recognized credential's registered `principal_id` does not match the
      Hello `principal_id`. For session namespace: accept `"socket-trust"` per
      enforcement policy (D1c); no store entry created, routing uses claimed
      `principal_id`. For `@EXTERNAL` and `@RELAY`: always require a recognized
      token.
- [ ] 1.10 Add relay-level `require_session_credentials` setting (boolean,
      default `false`) and thread through to Hello handling. Setting lives at
      relay level, not bundle level: with a single relay socket all connections
      share one transport boundary, so per-bundle enforcement is meaningless
      (a client can claim any `principal_id` namespace). For Slice 1, wire as
      a CLI flag (`--require-credentials`) on `agentmux host relay`; migrates
      to `relay.toml` in the relay config OpenSpec.
- [ ] 1.11 Implement expiry-based pruning in the principal store: prune expired
      records on startup and on access.
- [ ] 1.12 Integration test: Hello with valid session credential →
      session registered with stable `principal_id`.
- [ ] 1.13 Integration test: Hello with `"socket-trust"` + enforcement off →
      session accepted, no principal store entry created.
- [ ] 1.14 Integration test: Hello with `"socket-trust"` + enforcement on →
      typed error response, session not registered.
- [ ] 1.15 Integration test: Hello with unrecognized credential →
      typed error regardless of enforcement setting.
- [ ] 1.16 Integration test: reconnect with same credential →
      same `principal_id` returned from store.
- [ ] 1.17 Integration test: application principal Hello (`@EXTERNAL` token
      registered via `new peer`) → application principal type assigned,
      `IdentityIntrospect` right granted.
- [ ] 1.18 Integration test: Hello with valid credential but mismatched
      `principal_id` → typed error, session not registered.
- [ ] 1.19 Integration test: `new peer` creates principal in store, returns
      PSK; subsequent Hello with that PSK resolves to the correct `principal_id`.
- [ ] 1.20 Integration test: `change psk` updates store; new PSK accepted in
      Hello; old PSK rejected.

## Slice 2 — Introspection and Revocation Surface (depends on slice 1)

- [ ] 2.1 Implement `change psk` revocation dispatch: when the principal store
      hash is replaced, send `runtime_identity_revoked` to any active session
      holding the old credential and close the connection.
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
      stream-event carrier when a principal is revoked (via `change psk` or
      explicit revocation).
- [ ] 2.8 Implement relay-side session teardown on expiry: emit typed error
      response frame (`runtime_identity_expired`) before closing the connection.
- [ ] 2.9 Integration test: application principal introspects active session
      → returns `principal_id`, `expires_at`, `verified: true`.
- [ ] 2.10 Integration test: session principal issues IdentityIntrospect
      → authorization denial, no identity data returned.
- [ ] 2.11 Integration test: `change psk` on active session → session receives
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
- [ ] 3.4 Thread `authenticated_identity` into the delivered message envelope
      on the recipient side (the message that lands on the recipient's stream,
      not just the sender-side acknowledgement). Recipients need the sender's
      verified identity to build authorization on top. Update the envelope schema
      and relay delivery path accordingly.
- [ ] 3.5 Integration test: Chat response for authenticated session includes
      `authenticated_identity` set to `principal_id`.
- [ ] 3.6 Integration test: Chat response for unauthenticated session omits
      `authenticated_identity`.
- [ ] 3.7 Integration test: delivered envelope on recipient stream includes
      `authenticated_identity` from the sender's verified `principal_id`.
