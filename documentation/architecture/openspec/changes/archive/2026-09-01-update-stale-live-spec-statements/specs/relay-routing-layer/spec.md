## MODIFIED Requirements

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

The filter operates at peer-relay granularity (the `<id>@RELAY` principal's
scope), and SHALL continue to do so. Distinguishing which originating principal
*inside* the peer relay is acting is not part of it: the origin identity is
carried across the boundary as `on_behalf_of` per the `cross-relay-routing`
capability's `Cross-Relay Sender Attribution Forwarding` requirement, and that
value is advisory — the receiving relay authenticates the peer relay, not the
foreign origin — so it SHALL NOT be consumed as an authorization input here.

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

#### Scenario: Attribution does not widen ingress

- **WHEN** a forwarded target operation carries an `on_behalf_of` naming an
  origin principal
- **AND** that origin is outside the peer relay principal's registered `scope`
- **THEN** the ingress filter still evaluates the peer relay principal's scope
  alone
- **AND** the attribution does not permit or deny any target
