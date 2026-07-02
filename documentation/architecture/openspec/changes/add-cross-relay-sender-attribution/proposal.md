# Change: Add cross-relay sender attribution via on_behalf_of

## Why

`add-outbound-peer-relay-routing` (relay/90) forwards cross-relay `Send`/`Raww`
but drops the *original* sender's identity at the boundary: the delivered
`incoming_message` envelope on the receiving relay attributes only the peer relay
(`<connect-as>@RELAY`) as the authenticated sender. A recipient cannot tell which
principal *inside* the peer relay actually sent the message. The reserved
`on_behalf_of` field exists precisely for this — the `relay-identity` Sender
Attribution Schema declares it reserved and requires implementations to "leave it
absent until its setting mechanism is specified" (design decision D5 of relay/90
explicitly deferred that mechanism). This change specifies the mechanism for the
cross-relay forwarding path, closing the attribution half of federation.

## What Changes

- The forwarding (origin) relay SHALL stamp `on_behalf_of` on a cross-relay
  `Send`/`Raww` with the originating requester's canonical authenticated identity
  (its `principal_id`), when that origin sender is a verified principal; it SHALL
  omit `on_behalf_of` when the origin sender is unauthenticated (socket-trust) —
  an unauthenticated origin cannot be attributed.
- The receiving relay SHALL carry the peer-supplied `on_behalf_of` value,
  **uninterpreted**, into the delivered `incoming_message` envelope (and `Send`/
  `Look` responses where the sender-attribution schema surfaces it), alongside
  `authenticated_identity`, which continues to reflect the authenticated peer
  relay principal.
- `on_behalf_of` stays **advisory and opaque**: it is asserted by the peer relay
  and is only as trustworthy as that peer. The receiving relay authenticates the
  peer relay, not the foreign origin principal, and SHALL NOT use `on_behalf_of`
  as an authorization input — the target-side ingress filter continues to gate
  solely on the peer relay principal's `scope`.
- Not **BREAKING**: `on_behalf_of` is already reserved in the response/envelope
  schemas, so this populates an existing field rather than adding a wire field;
  local (non-cross-relay) delivery continues to leave it absent.

## Impact

- Affected specs:
  - `relay-identity` — MODIFIED Sender Attribution Schema: the `on_behalf_of`
    setting mechanism is now specified for cross-relay forwarding (no longer
    "left absent until specified"). The trusted-host-supplied `on_behalf_of` on
    `IdentityIntrospect` records remains a separate, still-reserved setter.
  - `cross-relay-routing` — ADDED Cross-Relay Sender Attribution Forwarding;
    MODIFIED Cross-Relay Delivery Outcome Propagation (drops the "attribution out
    of scope / deferred" paragraph, pointing at the new requirement).
- Affected code: the cross-relay forward path (`handlers/send.rs` /
  `handlers/raww.rs` `forward_*_cross_relay`, the outbound request build and its
  forward context), and the receiving-side `incoming_message` envelope /
  delivery-event build; the MCP `send`/`look` response surfaces already carry the
  reserved field.
- Non-goals (deferred): per-origin-principal ingress filtering that *consumes*
  `on_behalf_of` (ingress stays peer-relay-granular, `todos/relay/100`-adjacent);
  the trusted-host-set `on_behalf_of` on introspection records; multi-hop
  attribution chaining (a target names exactly one peer; no re-forward).

## Review posture

This is contract-shaping across `relay-identity` and the federated
sender-attribution model (`extensions-protocol` `on_behalf_of`), not a
BE-only change. The proposal is the discuss-first artifact: it is committed and
shared for cross-lane review before any implementation slice begins.
