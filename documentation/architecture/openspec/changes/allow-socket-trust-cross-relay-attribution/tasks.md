## 1. Forwarding

- [x] 1.1 Stamp `on_behalf_of` from the identity the connection was admitted
      under, at the cross-relay forwarding site in `src/relay/handlers/send.rs`
      and the corresponding site in `src/relay/handlers/raww.rs`. Both currently
      pass the origin's verified identity, which is `None` for socket-trust.
- [x] 1.2 Carry the admitted identity on the connection binding if it is not
      already available at those sites. The Hello path derives
      `authenticated_identity` as `store_backed.then(|| principal_id)`; the value
      needed here is the claimed `principal_id` regardless of `store_backed`, so
      it is a separate field rather than a widening of that one.
- [x] 1.3 Leave `authenticated_identity` derived from `store_backed` alone. It
      stays absent for a socket-trust session.
- [x] 1.4 Leave the non-ingress guard untouched. It discards a requester-supplied
      `on_behalf_of` rather than refusing the request — `send.rs` substitutes
      `None` for a non-ingress requester and `raww.rs` discards the field at
      destructuring. Attribution comes from Hello, not per request.
- [x] 1.5 Keep the two `on_behalf_of` paths distinct. The guard governs the value
      carried into *local* delivery, including a peer-forwarded one arriving
      through ingress; the change in 1.1 governs the value stamped on an
      *outbound* forwarded request. Attribution on locally delivered messages
      does not change.

## 2. Tests

- [x] 2.1 A socket-trust session's cross-relay `Send` is delivered with the
      composed `<origin>!<peer>` sender naming the identity it claimed at Hello,
      and that sender resolves as a reply target. The forwarding half is
      `cross_relay_send_stamps_on_behalf_of_from_socket_trust_origin`; the
      composition and resolution halves were already covered by
      `cross_relay_ingress_names_the_origin_and_the_asserting_peer` and
      `send_resolves_a_composed_cross_relay_sender_as_a_target`, which pass
      unchanged because the receiving side does not move.
- [x] 2.2 `authenticated_identity` is absent where `on_behalf_of` is present, for
      the same send.

      As originally written this task said "in the same delivered envelope",
      which is not observable: the `Send` response's `on_behalf_of` is the
      *ingress* field and stays `None` for a locally-originated send, while the
      new attribution appears only on the forwarded outbound request. The two
      fields surface on two surfaces of one operation, so the fixture returns
      both and the test asserts across them. Asserting either alone would still
      pass if a later change derived one field from the other.
- [x] 2.3 A credentialed session's attribution is unchanged. Requires
      provisioning a real credential in the fixture — the existing harness
      connects as socket-trust throughout, so a test that does not provision one
      is not exercising this case. Covered by the existing
      `cross_relay_send_stamps_on_behalf_of_from_authenticated_origin`, extended
      with the `authenticated_identity` assertion that makes it the paired
      control for 2.2.
- [x] 2.4 A cross-relay `Send` carrying a requester-supplied `on_behalf_of` is
      not refused, and the forwarded request carries the Hello identity rather
      than the supplied value. One request, so both outcomes are observable
      together; asserting a refusal and a forwarded attribution on the same
      request would be asserting two mutually exclusive results.
- [x] 2.5 `tests/integration/session_relay_stream/on_behalf_of.rs`'s existing
      `local_send_drops_self_asserted_on_behalf_of` still passes unchanged. It
      pins that a non-ingress requester's supplied value never reaches a locally
      delivered envelope, which this change must not disturb — the outbound
      stamping path is a different site from the guard it asserts.
- [x] 2.6 A peer that omits `on_behalf_of` still produces the peer-principal
      fallback, and a reply to it is refused with
      `validation_unsupported_namespace` in both the plain and bang-path forms.
      Covered by the existing
      `cross_relay_ingress_without_an_origin_names_the_peer_qualified_once`,
      `send_rejects_a_cross_relay_target_in_a_non_routable_namespace`, and
      `look_rejects_cross_relay_target_as_unsupported`.
- [x] 2.7 A socket-trust session's cross-relay `Raww` forwards the identity it
      claimed at Hello. `Raww` forwards on its own branch ahead of the local
      delivery spine and stamps attribution independently of `Send`, so a revert
      or miswire there stayed green under every other test in this list. Covered
      by `cross_relay_raww_stamps_on_behalf_of_from_socket_trust_origin`, which
      needed a stub peer answering a `raww`-shaped response and a `raww = "all"`
      control on the shared cross-relay fixture policy — the fixture granted
      `send` scope only.
- [x] 2.8 Teeth-check 2.1 through 2.7 individually by reverting the change and
      confirming that each named assertion is the one that fails. A loop stops at
      its first failure and proves nothing about the assertions after it.

      Three mutations were needed, because no single revert exercises all of
      them. Reverting the `Send` forwarding site to the verified identity failed
      the three `Send` attribution assertions individually under
      `--no-fail-fast`, left the store-backed control passing, and left the
      `Raww` test passing — which is what shows the two forwarding branches are
      separately covered. Reverting only the `Raww` site failed only the `Raww`
      test. Neither revert reaches 2.2's `authenticated_identity` assertion,
      which sits after a failing one in the same test, so a third mutation
      deriving `authenticated_identity` from the claimed id regardless of
      `store_backed` confirmed that assertion fails on its own.

## 3. Documentation

- [x] 3.1 Record in `src/relay/README.md`, alongside the existing cross-relay
      attribution material, that attribution follows admission: the forwarded
      `on_behalf_of` is the identity the origin was admitted under, and
      `require-session-credentials` decides which identities are admitted rather
      than whether attribution happens.
- [x] 3.2 Note what the setting now carries downstream: cross-relay messages
      carry origin attribution in either mode, and the setting determines whether
      the attributed identity was verified against the principal store or
      accepted as the session's own claim. Stated that way round — an operator
      who reads it as a switch over whether attribution happens would draw the
      wrong conclusion from leaving it at its default.

      Recorded in `documentation/usage/maintainer-configuration-guide.md`, which
      is the repo-owned home for `relay.toml` key documentation. The deployed
      `relay.toml` itself is an operator-owned artifact outside this repository
      and is not modified here; its inline comment is the operator's to update if
      they want the note in both places.
