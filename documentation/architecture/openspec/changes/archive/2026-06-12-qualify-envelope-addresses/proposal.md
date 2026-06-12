# Change: Qualify envelope addresses with canonical principal ids

## Why

Delivered envelopes showed bare session ids in From/To/Cc
(`<session:master>`), so a recipient in another bundle could not derive the
sender's reply address without out-of-band knowledge (todos/relay/83). Worse,
co-recipients in other namespaces were dropped from the delivered envelope
entirely because the recipient list each delivery task carried was scoped to
its own per-namespace delivery group (issues/relay/25). Both were observed in
the 2026-06-10 cross-bundle smoke test.

## What Changes

- Every delivery task carries the full recipient list across all delivery
  groups as canonical `session@namespace` ids.
- Envelope From/To/Cc identity tokens carry canonical principal ids; display
  names still come from the delivery bundle's configuration, and co-recipients
  outside that bundle fall back to the canonical id alone.
- UI stream `incoming_message` events pass the canonical co-recipient list
  through unchanged instead of re-qualifying entries with the delivery
  bundle's namespace.
- The `relay.send.envelope.metadata` inscription's `sender_session`,
  `target_sessions`, and `cc_sessions` fields carry canonical ids.

## Impact

- Affected specs: `pane-envelope` (modified: Address Identity Format, CC
  Informational Semantics)
- Affected code: `src/relay/handlers/send.rs`, `src/relay/handlers/raww.rs`,
  `src/relay/context.rs`, `src/relay/delivery/dispatch/payload.rs`,
  `src/relay/delivery/ui_delivery.rs`, `src/envelope.rs`
- The `<session:...>` token format is unchanged; only its content becomes the
  canonical form.
