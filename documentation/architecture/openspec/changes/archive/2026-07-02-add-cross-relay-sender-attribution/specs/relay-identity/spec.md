## MODIFIED Requirements

### Requirement: Sender Attribution Schema

Relay Send and Look responses SHALL include an `authenticated_identity` field
when the requesting session has a verified `principal_id`. The field SHALL
carry the stable `principal_id` of the sender, not the ephemeral `session_id`.

If the sender's session carries an `on_behalf_of` claim supplied by an
authenticated intermediary — a trusted host, or a peer relay forwarding on behalf
of an origin principal (see the `cross-relay-routing` capability's Cross-Relay
Sender Attribution Forwarding requirement) — the relay SHALL stamp and carry that
claim in the response. The relay SHALL NOT interpret the `on_behalf_of` value; it
is an opaque intermediary-supplied string and SHALL NOT be used as an
authorization input. Consumers SHALL read `on_behalf_of` in the context of the
accompanying `authenticated_identity` (the intermediary that asserted it), not as
a globally resolvable principal id.

Sessions without a verified principal SHALL omit the `authenticated_identity`
field rather than populate it with a self-asserted value.

The MCP send and look response schemas SHALL surface `authenticated_identity`
when present. The `on_behalf_of` field is optional in the response and envelope
schemas. Its setting mechanism for cross-relay forwarding is specified by the
`cross-relay-routing` capability; implementations SHALL leave `on_behalf_of`
absent unless it is set by a specified mechanism (the trusted-host-supplied
`on_behalf_of` on `IdentityIntrospect` records remains a separate, still-reserved
setter).

#### Scenario: Authenticated sender shows principal_id in response

- **WHEN** a Send or Look response is issued for a session with a verified
  principal
- **THEN** the response includes `authenticated_identity` set to the session's
  `principal_id`

#### Scenario: Unauthenticated sender omits attribution field

- **WHEN** a Send or Look response is issued for a session without a verified
  principal
- **THEN** the response does not include `authenticated_identity`

#### Scenario: Authenticated sender's identity carried in delivered envelope

- **WHEN** a Send is dispatched from a session with a verified principal
- **THEN** each UI-stream recipient's `incoming_message` stream event includes
  `authenticated_identity` set to the sender's `principal_id`

#### Scenario: Socket-trust sender omitted from delivered envelope

- **WHEN** a Send is dispatched from a socket-trust session
- **THEN** the `incoming_message` stream event does not include
  `authenticated_identity`

#### Scenario: Peer-relay-forwarded sender carried as on_behalf_of

- **WHEN** a Send is delivered on the receiving relay from a peer relay principal
  that forwarded it on behalf of a verified origin principal
- **THEN** the `incoming_message` envelope includes `authenticated_identity` set
  to the peer relay principal
- **AND** includes `on_behalf_of` set to the origin principal's canonical id
  supplied by the peer, carried without interpretation
