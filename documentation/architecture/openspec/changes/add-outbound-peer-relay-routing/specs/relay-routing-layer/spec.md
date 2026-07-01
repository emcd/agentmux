## ADDED Requirements

### Requirement: Cross-Relay Target Classification

The routing resolution stage SHALL recognize the cross-relay bang-path target
notation `<session_id>@<bundle_name>!<relay_id>` for the delivery operations
`Send` and `Raww`. The `!<relay_id>` suffix SHALL be parsed before the
`@<namespace>` split; `<relay_id>` is the bare id portion of a configured
`[[peers]]` entry (no `@RELAY` suffix). A target carrying a `!<relay_id>` suffix
SHALL be classified as a **cross-relay target** carrying the peer `relay_id` and
the foreign `session_id@bundle_name`.

Classification SHALL remain configuration-free: the resolution stage SHALL NOT
consult `[[peers]]` or any catalog to classify a cross-relay target. The
existence of the named peer is a delivery-time concern validated by the operation
body, not the resolver — mirroring how an unknown local bundle surfaces at
delivery rather than resolution.

A cross-relay target is cross-namespace with respect to the requester's home
namespace by construction, so it SHALL classify at the `all` scope tier. The
origin-side authorization stage is unchanged: the requester's configured
`send` / `raww` scope MUST reach `all` for the operation to be authorized on the
originating relay.

#### Scenario: Cross-relay Send target classified from bang-path

- **WHEN** the relay receives a `Send` targeting `claude@myapp!peer-relay`
- **THEN** the resolution stage classifies a cross-relay target with
  `relay_id = peer-relay` and foreign principal `claude@myapp`
- **AND** does so without consulting `[[peers]]` or the bundle catalog

#### Scenario: Cross-relay target requires origin all-tier authorization

- **WHEN** a session issues a `Send` or `Raww` to a `!<relay_id>` target
- **AND** the requester's configured scope for the operation is `home` or narrower
- **THEN** the relay returns `authorization_forbidden`

#### Scenario: Malformed bang-path rejected at resolution

- **WHEN** a target carries a `!<relay_id>` suffix with an empty `<relay_id>` or
  a missing `@<bundle_name>` segment
- **THEN** the resolution stage rejects it with a structured validation error
  without consulting configuration

### Requirement: Cross-Relay Target Ingress Filter

The authorization stage SHALL apply a target-side ingress filter, in addition to
the uniform scope-tier check, whenever a target-addressed operation's requester
is a relay principal (`<id>@RELAY`) — i.e. an inbound request forwarded by a peer
relay. The ingress filter SHALL authorize each resolved target against the peer
relay principal's registered `scope` — the value recorded on the principal store
record when the peer credential is registered via `new peer <id>@RELAY`,
evaluated with the existing scope-permits check:

- A target whose namespace or canonical principal id is covered by the peer's
  `scope` SHALL be permitted.
- The posture SHALL be **deny-by-default**: a peer principal with an empty or
  absent `scope` covers no target, and any target outside the scope SHALL be
  rejected with `authorization_forbidden` carrying an ingress-denied detail.

The ingress filter SHALL be evaluated at the shared `authorize_route` stage — the
single seam every target operation passes through — not within individual
operation bodies, and SHALL preserve the existence-before-authorization ordering
(`validation_unknown_target` before `authorization_forbidden`). The ingress
filter composes with, and does not replace, the origin-side capability model: it
is an independent authority exercised by the receiving trust domain.

This slice filters at peer-relay granularity (the `<id>@RELAY` principal's
scope). Distinguishing which originating principal *inside* the peer relay is
acting is out of scope; carrying the original sender identity across the boundary
(the reserved `on_behalf_of` field) is deferred to a follow-on, so this slice
gates solely on the peer relay principal.

#### Scenario: In-scope cross-relay target accepted

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@myapp`
- **AND** the peer principal's registered `scope` covers `myapp`
- **THEN** the ingress filter permits the target and delivery proceeds

#### Scenario: Out-of-scope cross-relay target denied

- **WHEN** a peer relay principal issues a forwarded `Send` to `claude@secret`
- **AND** the peer principal's registered `scope` does not cover `secret`
- **THEN** the relay returns `authorization_forbidden` with an ingress-denied
  detail

#### Scenario: Peer with no scope reaches nothing

- **WHEN** a peer relay principal with an empty or absent `scope` issues a
  forwarded target operation
- **THEN** the relay returns `authorization_forbidden` for every target
  (deny-by-default)
