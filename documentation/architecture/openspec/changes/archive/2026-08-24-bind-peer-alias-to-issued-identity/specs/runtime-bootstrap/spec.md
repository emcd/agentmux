## MODIFIED Requirements

### Requirement: Outbound Peer Relay Configuration

Relay configuration SHALL support top-level `[[peers]]` entries that define
outbound peer relay routing. `[[peers]]` is purely an outbound routing table; it
carries no inbound authorization. Each peer entry SHALL carry:

- `alias`: a required non-empty string — this relay's **local** name for the
  peer, which SHALL be the bare relay id of the identity this relay issued that
  peer (see the `cross-relay-routing` capability's Peer Naming Authority
  requirement). It serves as the peer's `<alias>` in cross-relay bang-path
  addressing (`<session>@<bundle>!<alias>`) and as the `<alias>` in the
  credential file path. It is internal to this relay and never presented to the
  peer. Grammar: a bare relay id (non-empty; no `@`, `!`, or path separator).
- `address`: a required outbound endpoint. In this slice `address` SHALL be an
  **absolute filesystem path** to a Unix domain socket (same-host peers), the
  transport the relay presently serves. A non-absolute value, or a `host:port`
  TCP-style endpoint, SHALL be rejected at startup and pre-flight validation with
  a structured error — the fail-fast counterpart of the remote/TCP non-goal,
  rather than deferring the failure to an unreachable-socket delivery outcome. A
  `host:port` TCP endpoint is the documented future shape once the relay gains a
  TCP listener and is not yet a supported target.
- `connect-as`: a required non-empty bare relay id — the identity this relay
  presents to the peer (`<connect-as>@RELAY`), determined by the peer (see Relay
  Cross-Relay Presented Identity).

`alias` and `connect-as` SHALL remain independent values naming opposite
directions of the same relationship, and neither SHALL be derived from the other.
Nothing SHALL key on `connect-as`: peers determine independently what identity
they issue this relay, so two peers may issue colliding values.

Every peer named in `[[peers]]` SHALL also be registered on this relay via
`new peer <alias>@RELAY`, including a peer this relay only dials and never
receives from. The registration is what gives the alias a referent; without it
the alias names nothing this relay issued, and the entry cannot be distinguished
from one whose alias is simply wrong.

Startup and pre-flight configuration validation SHALL verify that each entry's
`<alias>@RELAY` is present in the principal store as a relay principal, and SHALL
fail with a structured validation error naming the offending `alias` when it is
not. The check SHALL be unconditional across entries: it SHALL NOT treat a
missing store record as evidence that the peer is dial-only, because a missing
record is indistinguishable from a mistyped or stale alias, and reading it as the
former would accept exactly the misconfiguration this requirement exists to
reject. The check SHALL be against the store record's existence and principal
type only. It SHALL NOT compare the outbound credential at
`<state-root>/peers/<alias>.psk` against the store record's credential hash:
those are credentials issued in opposite directions — the peer issued this relay
the former, this relay issued the peer the latter — and requiring them to agree
would assert a relationship the data model does not record.

Inbound authorization for a peer relay — what an inbound request carried by that
peer may reach on this relay — is NOT configured here. It is the `scope` recorded
on the peer relay principal's store record when its credential is registered via
`new peer <id>@RELAY`, and is read by the ingress filter (see the
`relay-routing-layer` capability). A relay that only receives from a peer
therefore needs no `[[peers]]` entry for it — only a registered credential.

Unknown peer entry fields SHALL fail startup and pre-flight configuration
validation with structured validation errors. Peer entries SHALL NOT contain raw
PSK material; raw peer relay PSKs SHALL remain owner-only state artifacts at
`<state-root>/peers/<alias>.psk` (mode 0600), while the principal store records
credential hashes. An absent or unreadable credential at that path SHALL fail the
affected delivery with a typed outcome naming the path. The relay SHALL NOT
consult any other path for a peer credential.

The relay SHALL NOT open an outbound peer connection at startup solely because a
peer entry exists; connections are established lazily on first cross-relay
delivery to that peer (see the `cross-relay-routing` capability). A peer whose
endpoint is unreachable at startup SHALL NOT block or fail relay startup.

#### Scenario: Validate outbound peer entry

- **WHEN** `relay.toml` contains a `[[peers]]` entry with a non-empty `alias`, an
  absolute `address` Unix socket path, and a non-empty bare-id `connect-as`
- **AND** `<alias>@RELAY` is a registered relay principal
- **THEN** configuration validation accepts the entry
- **AND** relay startup does not attempt an outbound peer connection

#### Scenario: Reject a peer alias naming no issued identity

- **WHEN** a `[[peers]]` entry's `<alias>@RELAY` is absent from the principal
  store, or is present with a principal type other than relay
- **THEN** relay startup fails with a structured validation error naming the
  offending `alias`
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Require registration for a peer this relay only dials

- **WHEN** a `[[peers]]` entry names a peer this relay dials but never receives
  from
- **AND** no `<alias>@RELAY` principal is registered for it
- **THEN** relay startup fails with a structured validation error naming the
  offending `alias`

#### Scenario: Accept an alias differing from connect-as

- **WHEN** a `[[peers]]` entry's `alias` and `connect-as` hold different values
- **AND** `<alias>@RELAY` is a registered relay principal
- **THEN** configuration validation accepts the entry

#### Scenario: Reject non-absolute or TCP-style peer address

- **WHEN** a `[[peers]]` entry's `address` is not an absolute path — e.g. a
  `host:port` TCP endpoint or a relative path
- **THEN** relay startup fails with a structured validation error naming the
  `peers.address` field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject peer entry missing alias or connect-as

- **WHEN** a `[[peers]]` entry omits (or leaves empty) `alias` or `connect-as`
- **THEN** relay startup fails with a structured validation error naming the
  offending field
- **AND** `agentmux check configuration` reports the same invalid artifact
