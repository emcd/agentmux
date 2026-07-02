# Change: Add outbound peer relay routing via [[peers]] config

## Why

`add-identity-federation` (Slice 1) landed inbound peer-relay authentication: a
peer relay authenticates via the standard Hello path as a `<id>@RELAY` principal
whose PSK hash and scope live in the relay-level principal store. It
**deliberately deferred** the outbound half (design decision D9): the ability for
this relay to *initiate* a connection to a configured peer, forward a `Send` or
`Raww` addressed to a foreign principal, and report the delivery outcome back to
the originating requester. `add-relay-toml-configuration` then landed `[[peers]]`
as a schema-only placeholder (a required non-empty `address`, validated but
routing-inert).

This change delivers that deferred outbound slice, closing the cross-relay
message-forwarding loop. It is contract-shaping: it activates the `[[peers]]`
table, introduces cross-relay target addressing on the routing spine, adds an
outbound connection manager, and — because the destination is a *foreign trust
domain* — establishes the target-side ingress filter that `ideas/relay/2`
identifies as load-bearing at exactly this boundary.

## What Changes

- **Activate `[[peers]]` config** (**BREAKING** for the placeholder contract):
  peer entries gain `alias` (this relay's local name for the peer — the
  bang-path selector and credential filename stem) and `connect-as` (the identity
  this relay presents to that peer, composed as `<connect-as>@RELAY`), keeping the
  required `address` — which in this slice is a same-host Unix domain socket path
  (the relay serves only a Unix socket today; TCP is future). The presented
  identity is configured **per peer**, not relay-wide: the receiving relay
  determines the identity it expects (via its own `new peer`), so two peers may
  issue this relay different — or colliding — identities and no single relay-wide
  identity exists. `[[peers]]` stays outbound-only and carries no scope: inbound
  cross-relay authorization is the scope set by the existing
  `new peer <id>@RELAY --scope`, read by the ingress filter. The relay reads the
  outbound PSK from the well-known peer credential path and opens/maintains an
  outbound connection per configured peer. A peer entry now changes routing
  behavior.
- **Cross-relay target addressing**: the routing resolution stage recognizes the
  bang-path notation `<session>@<bundle>!<relay_id>` (pre-defined in
  `add-identity-federation` D6) and classifies such a target as *cross-relay*,
  routing it to the named peer's outbound connection instead of the local bundle
  catalog. Applies to `Send` and `Raww`.
- **Outbound connection management**: dial the peer `address`, present Hello as
  the per-peer `<connect-as>@RELAY` principal that peer issued this relay, with
  the peer PSK, and maintain the connection with reconnect/backoff. Unreachable or
  unauthenticated peers fail the affected delivery with a typed outcome; they do
  not fail relay startup.
- **Delivery-outcome propagation**: a cross-relay `Send`/`Raww` returns a
  delivery outcome derived from the peer relay's response (delivered / rejected /
  peer-unavailable), surfaced to the originating requester through the same
  outcome channel as local delivery.
- **Target-side ingress filter** (deny-by-default): on the *receiving* relay, an
  inbound request carried by a `<id>@RELAY` peer principal is authorized against
  that peer principal's registered `scope` at the shared `authorize_route` seam.
  A foreign origin reaches only what the peer's scope permits; an unscoped or
  unregistered peer reaches nothing. Per-origin-principal allowlist grammar (which
  *originating* principal inside the peer relay is acting) stays future work; this
  slice establishes the posture, the peer-relay-granular check, and the seam.

## Impact

- Affected specs:
  - `runtime-bootstrap` — MODIFIED `[[peers]]` from placeholder to active
    outbound-only peer config (`alias`, `address` Unix socket path, `connect-as`),
    MODIFIED Relay Configuration File + Precedence to admit the expanded peer
    fields, and ADDED a Relay Cross-Relay Presented Identity requirement (per-peer
    `connect-as`).
  - `relay-routing-layer` — ADDED: cross-relay target classification (bang-path)
    and the target-side ingress filter at the authorization stage.
  - `cross-relay-routing` — ADDED (new capability): outbound connection
    management, cross-relay `Send`/`Raww` forwarding, and delivery-outcome
    propagation.
  - `relay-identity` — deliberately **not** affected: cross-boundary sender
    attribution (setting the reserved `on_behalf_of` field) is deferred to a
    follow-on, leaving the reserved-field contract intact.
- Affected code: `src/relay/authorization/loading.rs` (peer config),
  `src/relay/routing.rs` + `src/relay/handlers/routed.rs`/`send.rs`/`raww.rs`
  (classification, forwarding), `src/relay/authorization/checks.rs`
  (`authorize_route` ingress seam), a new outbound-connection module under
  `src/relay/`, `src/relay/identity.rs` (peer credential path load),
  `data/configuration/relay.toml` (documented peer fields), and the relay README.
- Alpha posture: no backwards-compatibility shim for the placeholder `[[peers]]`
  shape; the schema changes in place.
