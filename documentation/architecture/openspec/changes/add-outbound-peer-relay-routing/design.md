# Design: Outbound peer relay routing

## Context

This slice completes the cross-relay federation arc begun by
`add-identity-federation` (archived 2026-06-03). That change established, and this
one relies on:

- **Relay principals** (`<id>@RELAY`): a peer relay authenticates via the
  standard Hello path; its PSK hash and a `scope` field live in the relay-level
  principal store. Inbound verification already works.
- **Cross-relay address notation** (D6, pre-defined but unused):
  `<session_id>@<bundle_name>!<relay_id>`, UUCP bang-path style, where
  `<relay_id>` is the bare id portion of a `[[peers]]` entry (no `@RELAY`
  suffix). Example: `claude@myapp!peer-relay`.
- **`[[peers]]` deferral** (D9): relay-level config, no PSK in TOML. (D9
  anticipated a `scope` field on `[[peers]]`; this proposal instead keeps
  `[[peers]]` outbound-only and sources inbound scope from `new peer` — see D1.)
- **Inbound peer scope already shipped**: `new peer <id>@RELAY --scope` records a
  `scope` on the peer relay principal's store record, and `scope_permits(...)` is
  the existing check that gates access against it (used today for trusted-host
  introspection). The ingress filter reuses both.
- **Peer credential path** (D11): the outbound PSK Relay B uses to reach Relay A
  lives at `<state-root>/peers/<alias>.psk`, mode 0600, base64 (no-pad)
  32-byte CSPRNG output. The store keeps only hashes; TOML never carries secrets.

The trust-boundary framing driving the ingress filter is `ideas/relay/2` and
`src/relay/README.md` → "Authorization model: origin-side capability, no
target-side filter": intra-relay is one trust domain (origin capability
suffices); cross-relay is two domains, so the receiving relay cannot assume the
origin enforced anything and must apply its own ingress decision, deny-by-default.

## Goals

- Activate `[[peers]]` as an outbound routing table without weakening the
  fail-fast startup contract for malformed config.
- Route `Send`/`Raww` to `!<relay_id>` targets through a maintained outbound
  connection, reusing the existing route/authorize/execute spine rather than a
  parallel path.
- Propagate a truthful delivery outcome back to the originating requester,
  distinct from local-delivery and from relay-unavailable transport failures.
- Establish the target-side ingress filter at the one seam every target
  operation already passes through, deny-by-default, at peer-relay granularity.

## Non-Goals

- **Per-origin-principal ingress allowlists.** The receiving relay filters at
  peer-relay granularity (the `<id>@RELAY` principal's `scope`), not per
  originating session inside the peer. Finer grammar is future work
  (`ideas/relay/2`).
- **`Look`/`List` across relays.** This slice forwards `Send`/`Raww` (the
  fire-and-forward delivery operations). Cross-relay inspection/enumeration —
  which needs a request/response round-trip and snapshot semantics over the
  boundary — is an anticipated follow-on: cross-relay `List` for discovering a
  peer's reachable target namespaces/principals is likely needed early
  (`todos/relay/100`), with cross-relay `Look` behind it.
- **Remote / TCP peering.** The relay presently serves only a Unix domain socket,
  so `address` is a same-host socket path this slice; a `host:port` TCP endpoint
  (and the WAN-federation topology it enables) awaits a TCP listener and is out of
  scope here.
- **Declarative (infra-as-code) inbound scope.** Inbound scope is set
  imperatively via `new peer <id>@RELAY --scope`; whether to also support a
  declarative, version-controllable inbound-scope source (a separate inbound
  table, distinct from the outbound `[[peers]]`) is deferred (`todos/relay/101`).
- **Multi-hop / transitive peering.** A target names exactly one peer
  (`!<relay_id>`); the receiving relay does not re-forward.
- **Richer credential / verification models.** PSK-via-`new peer` is the credential
  model this slice uses. Token- or key-based schemes (OIDC/JWT, SAML, DID/pubkey)
  are future work; the Hello `identity_token` + verify-by-credential-partition
  seam keeps them pluggable without reshaping routing.
- **Cross-boundary sender attribution.** Carrying the *original* sender identity
  across the relay boundary (setting the reserved `on_behalf_of` field) is
  deferred (see D5). This slice leaves the `relay-identity` reserved-field
  contract untouched; the receiving relay's delivered envelope reflects the peer
  relay as the authenticated sender. A follow-on that specifies the
  `on_behalf_of` setting mechanism (with its `relay-identity` delta) adds full
  attribution.

## Decisions

### D1 — `[[peers]]` is outbound-only: `alias` + `address` + `connect-as`

A peer entry is:

```toml
[[peers]]
alias = "peer-relay"              # this relay's LOCAL name for the peer
address = "/run/agentmux/peer-relay.sock"   # same-host Unix socket path
# address = "10.0.0.7:7420"       # future shape (needs a TCP listener; not yet)
connect-as = "west"               # identity this relay presents: <connect-as>@RELAY
```

`alias` is this relay's **local** name for the peer: it is the `<relay_id>` used
in bang-path targets (`<session>@<bundle>!<alias>`) and the `<alias>` stem of the
credential file (`<state-root>/peers/<alias>.psk`). It is internal to this relay
and never presented to the peer, and — because it is both the routable selector
and the credential path — it MUST be unique across `[[peers]]` entries; a
duplicate is rejected fail-fast at load. `connect-as` is the identity this relay
presents to that peer (composed as `<connect-as>@RELAY` in the outbound Hello),
determined by the peer; see D3. `address` is required and non-empty; in this
slice it is a Unix domain socket path (same-host), because the relay serves only a
`UnixStream` today — a `host:port` TCP endpoint is the documented future shape. To
keep the same-host-only contract honest, config validation requires an
**absolute** socket path and rejects non-absolute / `host:port` forms up front
(fail-fast), rather than letting a TCP-looking value slip through to an
unreachable-socket delivery. Unknown fields still fail startup/pre-flight
(`deny_unknown_fields`).

`[[peers]]` carries **no `scope`** (resolved with the operator, was Q1). Scope is
an *inbound* concept and `[[peers]]` is the *outbound* routing table; putting
scope there conflates the two directions and leaves a receive-only peer (which
has no `[[peers]]` entry) with nowhere to declare its scope. Instead, inbound
scope is the value already recorded on the peer relay principal's store record by
`new peer <id>@RELAY --scope` (shipped in `add-identity-federation`), and the
ingress filter reads it via the existing `scope_permits`. This is a single
source, decoupled per direction, and reuses shipped machinery. Whether to *also*
offer a declarative infra-as-code inbound-scope source (a separate inbound table)
is a deferred decision (`todos/relay/101`).

### D2 — Cross-relay classification lives in the resolution stage

`routing.rs::resolve_target` today classifies by `@<namespace>` suffix alone. A
`!<relay_id>` suffix is parsed *before* the `@<namespace>` split: a target
matching `<session>@<bundle>!<relay_id>` yields a new `ResolvedTarget` shape
carrying the peer relay id plus the foreign `session@bundle`. This keeps
classification config-free (it does not consult `[[peers]]`; existence of the
peer is a delivery-time concern, mirroring how local unknown bundles surface as
`validation_unknown_bundle` in the handler body, not the resolver).

A cross-relay target is, by construction, cross-namespace from the requester's
home, so it classifies at the `all` tier and the **origin-side** authorization is
unchanged: the requester needs `send`/`raww` at `all`. No new origin-side control.

### D3 — Outbound connection manager, lazy and self-healing

A per-peer outbound connection is established lazily on first cross-relay
delivery to that peer (not eagerly at startup — an unreachable peer must not
block boot, matching D9's placeholder posture that a peer entry alone opens
nothing). The manager:

- reads `<state-root>/peers/<alias>.psk` (fail the *delivery*, not startup,
  if absent/unreadable),
- dials `address` (a same-host Unix socket path), sends Hello as the peer entry's
  configured `<connect-as>@RELAY` principal — an identity the peer must have
  registered via `new peer <connect-as>@RELAY`,
- reconnects with jittered exponential backoff (reusing the client-side backoff
  shape from `todos/relay/67`),
- surfaces a typed `peer-unavailable` outcome while down rather than blocking.

The identity this relay presents is configured **per peer**, not relay-wide
(resolved, was Q2): the *receiving* relay determines the identity it expects (via
its own `new peer`), so two peers may issue this relay different — or colliding —
identities and no single relay-wide identity exists. Each `[[peers]]` entry
therefore carries a `connect-as` string naming the bare relay id that peer issued
this relay; the connector composes `<connect-as>@RELAY` and presents it in the
outbound Hello to that peer. `connect-as` is required and non-empty per peer, and
validated as a **bare** relay id (no `@` suffix — the relay composes `@RELAY`
itself, so `foo@RELAY` is rejected rather than becoming `foo@RELAY@RELAY` — no
`!`, no path separators), reusing the existing bare-local-part grammar. The PSK it
presents to a given peer is the one that peer issued to this relay, read from
`<state-root>/peers/<alias>.psk` (D11, keyed on the local `alias` since
`connect-as` need not be unique) — so the identity is config-driven and the secret
stays out of TOML. A relay that only *receives* from a peer needs no `[[peers]]`
entry for it and presents no identity to it. `[[peers]]` resolves from
`relay.toml` only (no CLI or environment override), like `[choices].pending-max`;
the delta modifies the existing Relay Configuration File and Relay Configuration
Precedence requirements accordingly so the expanded peer fields are known keys
rather than unknown-field rejections.

### D4 — Delivery-outcome propagation

Cross-relay `Send`/`Raww` is async like local delivery, but the outcome must
reflect the peer's response. The outbound request carries the origin
`request_id`; the peer's typed response (delivered / `authorization_forbidden` /
`validation_unknown_target` / …) maps onto the originating requester's
delivery-outcome channel, plus a `peer_unavailable` outcome for transport
failure. The requester can thus distinguish "peer rejected me" (ingress denied)
from "peer is down" from "delivered."

### D5 — Sender attribution across the boundary is deferred

Carrying the *original* sender identity across the relay boundary would mean
setting the reserved `on_behalf_of` field. The current `relay-identity` spec
declares `on_behalf_of` reserved and requires implementations to leave it absent
until its setting mechanism is specified. Introducing that mechanism is a
distinct contract change to the federated sender-attribution model
(`extensions-protocol-onbehalfof`) and would need its own `relay-identity` delta.

This slice therefore **defers** cross-boundary attribution: it does not set
`on_behalf_of`, leaves the `relay-identity` reserved-field contract untouched
(no delta, no affected-specs entry), and accepts that the receiving relay's
delivered envelope reflects the peer relay (`<id>@RELAY`) as the authenticated
sender under the existing attribution contract. A follow-on proposal specifies
the `on_behalf_of` setting mechanism and adds full original-sender attribution
(tracked: `todos/relay/99`); that is the seam a future per-origin-principal
ingress grammar would also read.

### D6 — Target-side ingress filter: deny-by-default at `authorize_route`

On the **receiving** relay, an inbound cross-relay `Send`/`Raww` arrives over a
connection whose principal is the peer relay (`<id>@RELAY`). It flows through the
same route/authorize/execute spine as any request. The ingress filter is an
addition inside `authorize_route`, gated on the requester being a relay principal:

- The peer relay principal's registered `scope` is consulted. Each requested
  target namespace/principal must be covered by the scope.
- **Deny-by-default**: an empty or absent scope covers nothing; an unregistered
  peer never authenticates in the first place. A target outside scope yields
  `authorization_forbidden` (ingress-denied variant in the error detail).
- This composes with, and is orthogonal to, the origin-side capability model: the
  origin relay already required `all` of its own requester; the receiving relay
  independently confirms the peer may reach the target. Two authorities, two
  trust domains — neither replaces the other.

Placing it in `authorize_route` (not in each handler) keeps it a single seam and
preserves the existence-before-authorization ordering the spine guarantees
(`validation_unknown_target` before `authorization_forbidden`). This is the exact
insertion point `src/relay/README.md` and `ideas/relay/2` call out.

## Resolved During Review (RG rounds + operator review)

The three questions this proposal originally left open were normative — the spec
deltas assert SHALLs that depend on them — so they are resolved here rather than
left for implementation:

- **Q1 — `scope` source of truth → RESOLVED (operator review).** `[[peers]]` is
  outbound-only and carries no `scope`; inbound scope is set imperatively by
  `new peer <id>@RELAY --scope` on the principal store record and read by the
  ingress filter via `scope_permits`. This avoids conflating inbound/outbound in
  one table. A declarative infra-as-code inbound-scope source is a deferred
  decision (`todos/relay/101`). See D1; reflected in the `runtime-bootstrap` and
  `relay-routing-layer` deltas.
- **Q2 — Outbound self-identity → RESOLVED (operator review of the reshape).**
  The presented identity is receiver-issued, so it is configured **per peer**:
  each `[[peers]]` entry carries a `connect-as` bare relay id and the connector
  presents `<connect-as>@RELAY` to that peer. There is no relay-wide identity — an
  earlier draft's top-level `relay-id` was a modeling error, since two peers may
  issue this relay different or colliding identities. See D3; reflected in the
  `runtime-bootstrap` (Relay Cross-Relay Presented Identity requirement) and
  `cross-relay-routing` deltas.
- **Q3 — New capability vs `session-relay` → RESOLVED (keep separate).** The
  `cross-relay-routing` capability stays separate rather than folding into the
  86-requirement `session-relay` spec (RG concurred).

## Alternatives Considered

- **Eager peer connections at startup.** Rejected: an unreachable peer would
  block or destabilize boot, contradicting D9's "a peer entry opens nothing"
  posture. Lazy + self-healing keeps startup independent of peer reachability.
- **Origin-side-only authorization (no ingress filter).** Rejected for
  cross-relay: the receiving relay is a separate trust domain and cannot assume
  the origin enforced anything (`ideas/relay/2`). Deny-by-default ingress is the
  load-bearing control at this boundary.
- **A dedicated peer-forward request frame.** Rejected: the unified Hello +
  existing `Send`/`Raww` request variants already carry everything needed; the
  peer relay is just another authenticated principal on the receiving side. No
  new wire frame.
