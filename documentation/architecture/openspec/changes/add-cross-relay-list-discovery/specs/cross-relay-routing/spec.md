## ADDED Requirements

### Requirement: Cross-Relay Discovery Forwarding

The origin relay SHALL forward namespace and principal discovery to a configured
outbound peer selected by local `alias`. It SHALL reuse the existing lazy peer
connection manager, credential path, and per-peer presented identity used by
cross-relay `Send` and `Raww`.

Forwarded requests SHALL NOT carry the origin-local alias, an onward relay
selector, or `on_behalf_of`. The peer SHALL author all returned namespace and
principal data from its local state. The origin SHALL NOT synthesize or rewrite
foreign namespaces, bundle ids, or principals.

#### Scenario: Forward namespace discovery

- **WHEN** the origin handles namespace discovery for alias `west`
- **THEN** it dials or reuses the configured peer connection for `west`
- **AND** forwards a discovery request with no onward relay selector

#### Scenario: Forward principal discovery

- **WHEN** the origin handles principal discovery for `myapp` on alias `west`
- **THEN** the peer request names namespace `myapp`
- **AND** carries no `on_behalf_of`

#### Scenario: Forwarded discovery omits origin-only selectors

- **WHEN** the origin forwards namespace or principal discovery to a peer
- **THEN** the wire request contains no origin-local relay alias or onward relay
  selector
- **AND** contains no `on_behalf_of`

#### Scenario: Unknown peer alias fails on origin

- **WHEN** discovery names an alias absent from local `[[peers]]`
- **THEN** the origin returns `validation_unknown_peer`
- **AND** does not attempt an outbound connection

### Requirement: Cross-Relay Discovery Outcome Propagation

Cross-relay discovery SHALL propagate peer-authored successes and typed errors
to the origin requester. Foreign namespace responses SHALL contain only
peer-authored namespace ids. Foreign principal responses SHALL contain canonical
`ListedBundle` entries with foreign ids and any `principals_partial` marker
unchanged.

Discovery SHALL reuse peer connection errors:

- `runtime_peer_credential_missing` for absent or unreadable credentials
- `runtime_peer_unavailable` for dial or authentication failure

These errors SHALL remain distinct from local `relay_unavailable`. Unlike
`Send`, discovery SHALL propagate typed relay errors directly rather than folding
them into `SendOutcome`.

#### Scenario: Propagate peer success

- **WHEN** the peer accepts a discovery request
- **THEN** the origin returns the peer-authored result without local synthesis

#### Scenario: Propagate peer authorization denial

- **WHEN** the peer rejects discovery with `authorization_forbidden`
- **THEN** the origin requester receives that typed denial unchanged

#### Scenario: Peer unavailable remains distinct

- **WHEN** the peer connection cannot be established
- **THEN** the origin returns `runtime_peer_unavailable`
- **AND** does not report local `relay_unavailable`
