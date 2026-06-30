## 1. Payload reshape

- [x] 1.1 Add `AddressIdentity::canonical_session_id(&self) -> &str` in
  `src/envelope.rs` with a doc comment stating it is the bare canonical form for
  machine-consumed delivery-event fields, never `render_address`.
- [x] 1.2 Replace `DeliveryParty` with `AddressIdentity` on
  `DeliveryMessage.sender/target/cc` in `src/transports/contract.rs`; delete
  `DeliveryParty` and `DeliveryParty::to_address`.
- [x] 1.3 Repoint `render_pane_envelope` to use the carried `AddressIdentity`
  values directly (drop the `to_address()` calls).
- [x] 1.4 Repoint relay delivery-task payload construction to populate
  `AddressIdentity` directly.

## 2. Delivery-event parity

- [x] 2.1 Update `src/transports/ui.rs` incoming_message build to emit
  `sender_session` / `cc_sessions` via `canonical_session_id()` (bare canonical),
  not `render_address`.
- [x] 2.2 (FE, co-landed in the same commit) Add the invariant test: with a
  fixture whose `display_name` differs from `session_name`
  (`session_name = "alice@bundle"`, `display_name = Some("Alice Cooper")`),
  assert the `incoming_message` event `sender_session == "alice@bundle"` (NOT
  `"Alice Cooper <session:alice@bundle>"`). The `cc` party MUST also carry a
  distinct `display_name` (e.g. `Some("Carol King")`) so the `cc_sessions`
  decoration guard bites as hard as the sender side. Use a separate,
  intent-named test (e.g.
  `ui_incoming_message_emits_bare_canonical_identity_never_decorated`) so the
  anti-regression purpose stays legible.

## 3. Specs

- [x] 3.1 MODIFIED `transport-abstraction` "Structured Delivery Message Payload":
  payload carries structured `AddressIdentity` per party.
- [x] 3.2 MODIFIED `session-relay` "Relay Stream Event Contract":
  `incoming_message` `sender_session`/`cc_sessions` MUST be bare canonical via
  the non-decorating accessor, never the decorating header form.
- [x] 3.3 MODIFIED `pane-envelope` "Address Identity Format": explicit EXEMPT
  scenario — the pane header keeps the decorating `Display Name <session:...>`
  form.

## 4. Gates

- [x] 4.1 `cargo fmt --check`
- [x] 4.2 `cargo clippy -- -D warnings`
- [x] 4.3 `timeout 300 cargo test`
