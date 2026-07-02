# Tasks: Cross-relay sender attribution via on_behalf_of

## 1. Origin-side forwarding (cross-relay-routing)

- [x] 1.1 Thread the originating requester's authenticated identity (its
      `principal_id`, or its absence for socket-trust) into the cross-relay
      forward context (the `Send` execution context and the `Raww` forward path).
- [x] 1.2 In `forward_send_cross_relay` / `forward_raww_cross_relay`, stamp
      `on_behalf_of` on the outbound forwarded request with that `principal_id`
      when the origin is verified; omit it when the origin is unauthenticated.

## 2. Receiving-side attribution (relay-identity + cross-relay-routing)

- [x] 2.1 Carry a peer-supplied `on_behalf_of` from the inbound request through
      the receiving-side request principal into the delivered `incoming_message`
      envelope build, uninterpreted, alongside `authenticated_identity` (the peer
      relay principal).
- [x] 2.2 Surface `on_behalf_of` on `Send`/`Look` responses per the
      sender-attribution schema where it is present.
- [x] 2.3 Confirm the ingress filter does NOT consult `on_behalf_of`
      (authorization stays peer-relay-scoped).

## 3. Schema / wire

- [x] 3.1 Populate the already-reserved `on_behalf_of` field on the
      `incoming_message` envelope + response schemas; no new wire field is
      introduced.

## 4. Tests

- [x] 4.1 Integration: authenticated origin → forwarded request carries
      `on_behalf_of`; delivered envelope shows the peer `authenticated_identity`
      plus the origin `on_behalf_of`.
- [x] 4.2 Integration: unauthenticated (socket-trust) origin → `on_behalf_of`
      omitted; delivered envelope attributes only the peer relay principal.
- [x] 4.3 Integration: `on_behalf_of` is not an ingress authorization input — an
      out-of-scope target is still denied regardless of the asserted value.
- [x] 4.4 Regression: local (non-cross-relay) delivery still omits
      `on_behalf_of`.

## 5. Docs

- [x] 5.1 Update `src/relay/README.md` sender-attribution / cross-relay notes to
      describe the per-peer `on_behalf_of` mechanism and its advisory posture.

## 6. Validation

- [x] 6.1 `openspec validate add-cross-relay-sender-attribution --strict`.
- [x] 6.2 `cargo fmt --check`, `cargo clippy`, and the wrapped test suite green.
