## 1. Forwarding

- [ ] 1.1 Stamp `on_behalf_of` from the identity the connection was admitted
      under, at the cross-relay forwarding site in `src/relay/handlers/send.rs`
      and the corresponding site in `src/relay/handlers/raww.rs`. Both currently
      pass the origin's verified identity, which is `None` for socket-trust.
- [ ] 1.2 Carry the admitted identity on the connection binding if it is not
      already available at those sites. The Hello path derives
      `authenticated_identity` as `store_backed.then(|| principal_id)`; the value
      needed here is the claimed `principal_id` regardless of `store_backed`, so
      it is a separate field rather than a widening of that one.
- [ ] 1.3 Leave `authenticated_identity` derived from `store_backed` alone. It
      stays absent for a socket-trust session.
- [ ] 1.4 Leave the non-ingress guard untouched. It discards a requester-supplied
      `on_behalf_of` rather than refusing the request — `send.rs` substitutes
      `None` for a non-ingress requester and `raww.rs` discards the field at
      destructuring. Attribution comes from Hello, not per request.
- [ ] 1.5 Keep the two `on_behalf_of` paths distinct. The guard governs the value
      carried into *local* delivery, including a peer-forwarded one arriving
      through ingress; the change in 1.1 governs the value stamped on an
      *outbound* forwarded request. Attribution on locally delivered messages
      does not change.

## 2. Tests

- [ ] 2.1 A socket-trust session's cross-relay `Send` is delivered with the
      composed `<origin>!<peer>` sender naming the identity it claimed at Hello,
      and that sender resolves as a reply target.
- [ ] 2.2 `authenticated_identity` is absent in the same delivered envelope where
      `on_behalf_of` is present. Asserting both together is what pins them as
      separately sourced; asserting either alone would pass against a
      simplification that collapsed them.
- [ ] 2.3 A credentialed session's attribution is unchanged. Requires
      provisioning a real credential in the fixture — the existing harness
      connects as socket-trust throughout, so a test that does not provision one
      is not exercising this case.
- [ ] 2.4 A cross-relay `Send` carrying a requester-supplied `on_behalf_of` is
      not refused, and the forwarded request carries the Hello identity rather
      than the supplied value. One request, so both outcomes are observable
      together; asserting a refusal and a forwarded attribution on the same
      request would be asserting two mutually exclusive results.
- [ ] 2.5 `tests/integration/session_relay_stream/on_behalf_of.rs`'s existing
      `local_send_drops_self_asserted_on_behalf_of` still passes unchanged. It
      pins that a non-ingress requester's supplied value never reaches a locally
      delivered envelope, which this change must not disturb — the outbound
      stamping path is a different site from the guard it asserts.
- [ ] 2.6 A peer that omits `on_behalf_of` still produces the peer-principal
      fallback, and a reply to it is refused with
      `validation_unsupported_namespace` in both the plain and bang-path forms.
      This path is now reachable only from a non-conforming or older peer, so it
      needs a test that constructs that case directly rather than relying on a
      local socket-trust send to produce it.
- [ ] 2.7 Teeth-check 2.1 through 2.6 individually by reverting the change and
      confirming that each named assertion is the one that fails. A loop stops at
      its first failure and proves nothing about the assertions after it.

## 3. Documentation

- [ ] 3.1 Record in `src/relay/README.md`, alongside the existing cross-relay
      attribution material, that attribution follows admission: the forwarded
      `on_behalf_of` is the identity the origin was admitted under, and
      `require-session-credentials` decides which identities are admitted rather
      than whether attribution happens.
- [ ] 3.2 Note in the deployed `relay.toml` comment for
      `require-session-credentials` what the setting now carries downstream:
      cross-relay messages carry origin attribution in either mode, and the
      setting determines whether the attributed identity was verified against
      the principal store or accepted as the session's own claim. State it that
      way round — an operator who reads it as a switch over whether attribution
      happens would draw the wrong conclusion from leaving it at its default.
