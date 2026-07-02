# Tasks: Outbound peer relay routing

## 1. Peer configuration (runtime-bootstrap)

- [x] 1.1 Extend `RawPeerEntry` / `PeerConfiguration` in
      `src/relay/authorization/loading.rs` with `alias` (required, this relay's
      local name for the peer) and `connect-as` (required, the bare relay id this
      relay presents as `<connect-as>@RELAY`), keeping `address` required
      non-empty. `[[peers]]` stays outbound-only (no `scope` field). Keep
      `deny_unknown_fields`.
- [x] 1.2 Validate peer entries at load: non-empty bare-id `alias` and
      `connect-as` (no `@`/`!`/path separators, reusing the bare-local-part
      grammar; reject qualified/malformed values naming the offending field);
      `address` an absolute Unix socket path — reject non-absolute or `host:port`
      TCP-style forms (naming `peers.address`). Fail startup and pre-flight with
      structured errors (extend the existing `peers.*` field-label diagnostics).
- [x] 1.3 Reject duplicate `peers.alias` across `[[peers]]` entries (the alias is
      the bang-path selector and credential filename stem, so it MUST be unique)
      with a structured error naming `peers.alias`; fail startup and pre-flight.
      Duplicate `connect-as` stays allowed (the presented identity is
      receiver-issued and may legitimately collide across peers).
- [x] 1.4 Update `data/configuration/relay.toml` template with documented
      `alias`/`address`/`connect-as` peer fields (kept commented / all-defaults),
      `address` shown as a Unix socket path with the TCP form as a future example.
- [x] 1.5 Update `src/relay/README.md` peer-placeholder note → active
      outbound-only peer config, plus the per-peer `connect-as` presented
      identity, the inbound-scope-via-new-peer model, and credential-path notes.

## 2. Cross-relay target classification (relay-routing-layer)

- [x] 2.1 Parse the `!<relay_id>` bang-path suffix in `routing.rs` before the
      `@<namespace>` split; add a cross-relay variant to `ResolvedTarget`
      carrying `relay_id` + foreign `session@bundle`.
- [x] 2.2 Classify a cross-relay target at the `all` tier (cross-namespace by
      construction); confirm origin-side `authorize_route` is unchanged.
- [x] 2.3 Keep classification config-free (no `[[peers]]` lookup in the
      resolver); unknown peer surfaces at delivery time, not resolution.

## 3. Outbound connection management (cross-relay-routing)

- [x] 3.1 New module under `src/relay/` for a per-peer outbound connection
      manager: lazy establishment on first delivery, jittered exponential
      backoff reconnect (reuse `ensure_connected` backoff shape).
- [x] 3.2 Read `<state-root>/peers/<alias>.psk` (fail the delivery, not
      startup, on absence/unreadable); dial `address` (Unix socket); Hello as the
      peer entry's configured `<connect-as>@RELAY` with the peer PSK.
- [x] 3.3 Route resolved cross-relay `Send`/`Raww` through the peer connection.

## 4. Delivery-outcome propagation (cross-relay-routing)

- [x] 4.1 Carry the origin `request_id` outbound; map the peer's typed response
      onto the originating requester's delivery-outcome channel.
- [x] 4.2 Add a `peer_unavailable` outcome distinct from local delivery outcomes
      and from `relay_unavailable`.
      (Cross-boundary sender attribution — setting `on_behalf_of` — is out of
      scope this slice; deferred to a follow-on with its `relay-identity` delta.)

## 5. Target-side ingress filter (relay-routing-layer)

- [x] 5.1 In `authorize_route` (`src/relay/authorization/checks.rs`), add the
      ingress gate for a requester that is a relay principal (`<id>@RELAY`):
      each target must be covered by the peer principal's registered `scope`
      (set via `new peer <id>@RELAY --scope`), reusing `scope_permits`.
- [x] 5.2 Deny-by-default: empty/absent scope covers nothing; out-of-scope
      target → `authorization_forbidden` with an ingress-denied detail.
- [x] 5.3 Preserve existence-before-authorization ordering
      (`validation_unknown_target` before `authorization_forbidden`).

## 6. Tests

- [x] 6.1 Unit: peer config load (valid `alias`/absolute-socket
      `address`/`connect-as`; missing `alias`; missing `connect-as`; empty
      `address`; non-absolute/`host:port` `address` rejected naming
      `peers.address`; unknown field incl. a rejected `scope` on `[[peers]]`;
      `alias`/`connect-as` rejected when qualified/malformed (e.g. `foo@RELAY`,
      contains `!`, or whitespace) naming the offending field; duplicate `alias`
      rejected naming `peers.alias`; duplicate `connect-as` across distinct
      aliases accepted).
- [x] 6.2 Unit: bang-path classification (cross-relay target → `all` tier,
      correct `relay_id` + foreign `session@bundle`; malformed bang-path).
- [x] 6.3 Unit/integration: ingress filter — in-scope target accepted,
      out-of-scope denied, absent-scope denied (deny-by-default).
- [x] 6.4 Integration: outbound delivery outcome propagation (delivered,
      ingress-denied, peer-unavailable) against a stub peer relay.
- [x] 6.5 Integration: unreachable peer at startup does not block boot; first
      delivery to it yields `peer_unavailable`.

## 7. Validation

- [x] 7.1 `openspec validate add-outbound-peer-relay-routing --strict`.
- [x] 7.2 `cargo fmt --check`, `cargo clippy`, and the wrapped test suite green.
