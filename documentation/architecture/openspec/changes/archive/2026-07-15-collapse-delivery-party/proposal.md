# Change: Collapse DeliveryParty onto AddressIdentity in the delivery payload

## Why

`refactor-render-in-transport-payload` (b7a195c) moved pane-envelope rendering
into the transports, but `DeliveryMessage` still carries each party as a
`DeliveryParty { session: String, display_name: Option<String> }` and converts
it to an `AddressIdentity` via `to_address()` at render time inside every coder
transport. `DeliveryParty` is structurally identical to
`crate::envelope::AddressIdentity` (`session` is `session_name` renamed), so the
conversion is a pure field rename and the parallel type is redundant. Carrying
`AddressIdentity` directly removes the per-transport conversion and one
duplicated type.

## What Changes

- Replace `DeliveryParty` with `crate::envelope::AddressIdentity` carried
  directly on `DeliveryMessage.sender`, `DeliveryMessage.target`, and
  `DeliveryMessage.cc`. Delete `DeliveryParty` and `DeliveryParty::to_address`.
  **BREAKING** (internal payload type; alpha, no compatibility shims).
- Add a non-decorating accessor `AddressIdentity::canonical_session_id(&self)
  -> &str` returning the bare canonical `session@namespace` id. The
  delivery-event path uses it; it sits beside `render_address` so the
  decorating/non-decorating contrast is legible at the call site.
- **Parity invariant (enforced by spec + co-landed test):** the
  `incoming_message` event's `sender_session` / `cc_sessions` fields MUST emit
  the bare canonical `session@namespace` form via the non-decorating accessor —
  never via `render_address` (which decorates to
  `Display Name <session:session_name>`). The pane-envelope header
  (`ManifestEnvelope` From/To/Cc) is **EXEMPT** and keeps `render_address`.
- State the exemption explicitly in the pane-envelope spec so the split is not
  left to inference.

## Impact

- Affected specs: `transport-abstraction` (Structured Delivery Message Payload),
  `session-relay` (Relay Stream Event Contract), `pane-envelope` (Address
  Identity Format).
- Affected code: `src/transports/contract.rs` (DeliveryMessage/DeliveryParty,
  render_pane_envelope), `src/transports/ui.rs` (incoming_message event build),
  `src/envelope.rs` (AddressIdentity accessor), relay delivery-task payload
  construction.
- No new transport→relay back-edge: `AddressIdentity` already lives in the
  shared `crate::envelope` module and `contract.rs` already imports it. This
  removes a parallel type and nudges toward the transport-decoupling end-state.
