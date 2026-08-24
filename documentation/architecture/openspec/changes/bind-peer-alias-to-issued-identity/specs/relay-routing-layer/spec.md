## MODIFIED Requirements

### Requirement: Cross-Relay Target Classification

The routing resolution stage SHALL recognize the cross-relay bang-path target
notation `<principal_id>!<relay_id>` for the delivery operations `Send` and
`Raww`. The `!<relay_id>` suffix SHALL be parsed before the `@<namespace>` split;
`<relay_id>` is the local `alias` of a configured `[[peers]]` entry (this relay's
own name for the peer, no `@RELAY` suffix). A target carrying a `!<relay_id>`
suffix SHALL be classified as a **cross-relay target** carrying the peer
`relay_id` and the foreign `<principal_id>`.

The foreign principal SHALL be accepted when it is a bundle-qualified session or
a relay-wide `@GLOBAL` principal, and not only the former. Cross-relay forwarding
attributes any verified requester, and a relay-wide user is one, so restricting
resolution to bundle sessions would make a correctly delivered sender unparseable
as a target and break the reply path the envelope form exists to provide.

The namespaces that name no routable recipient locally — the application and peer
relay partitions — SHALL continue to be rejected as unsupported, and an
unqualified principal SHALL continue to be rejected as such. Widening reaches the
principal kinds a conforming forwarding relay can attribute, not every string a
peer might assert; a delivered sender may carry an origin outside that set, and a
reply to it is expected to fail here rather than to resolve. A target SHALL also
be rejected when the `relay_id` is empty or itself contains a separator.

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

#### Scenario: Cross-relay target carrying a relay-wide origin

- **WHEN** the relay receives a `Send` targeting `operator@GLOBAL!peer-relay`
- **THEN** the resolution stage classifies a cross-relay target with
  `relay_id = peer-relay` and foreign principal `operator@GLOBAL`

#### Scenario: Cross-relay target requires origin all-tier authorization

- **WHEN** a session issues a `Send` or `Raww` to a `!<relay_id>` target
- **AND** the requester's configured scope for the operation is `home` or narrower
- **THEN** the relay returns `authorization_forbidden`

#### Scenario: Cross-relay target in a non-routable namespace still rejected

- **WHEN** the relay receives a `Send` targeting a principal qualified with the
  application or peer relay namespace and a `!<relay_id>` suffix
- **THEN** the resolution stage rejects it as an unsupported namespace

#### Scenario: Malformed bang-path rejected at resolution

- **WHEN** a target carries a `!<relay_id>` suffix with an empty `<relay_id>` or
  an unqualified principal
- **THEN** the resolution stage rejects it with a structured validation error
  without consulting configuration
