## MODIFIED Requirements

### Requirement: Address Identity Format

Header addresses SHALL support display names and canonical session identifiers
using:

- `Display Name <session:session@namespace>`

The `session:` identity token SHALL carry the canonical `session@namespace`
principal id, so a recipient in any namespace can derive a reply address from
the envelope alone. When the address names a sender attributed to a cross-relay
origin, the token SHALL instead carry the `<origin>!<peer-name>` form specified
by the `cross-relay-routing` capability, which is what makes a reply address
derivable in that case: a bare `session@namespace` id cannot name the relay the
origin lives on, and a reply derived from it would resolve against the receiving
relay instead. When an address has no configured display name in the delivery
bundle (for example a co-recipient in another namespace), the display portion
SHALL fall back to the canonical id.

This decorating form is REQUIRED for the pane-envelope From/To/Cc header and is
the only delivery surface that uses it. The pane header is EXEMPT from the
bare-canonical-identity rule that governs machine-consumed event fields (for
example the relay `incoming_message` event's `sender_session` / `cc_sessions`),
which carry the bare canonical `session@namespace` id without decoration, or the
cross-relay form where the sender is attributed to a peer.

#### Scenario: Render sender with display name

- **WHEN** sender display metadata is available
- **THEN** `From` header includes display name and `session:` identity token
- **AND** the identity token carries the canonical `session@namespace` id

#### Scenario: Render cross-namespace address without configured display name

- **WHEN** an address has no configured display name in the delivery bundle
- **THEN** the address renders the canonical id in both the display and
  identity positions

#### Scenario: Render a cross-relay sender as a reply-derivable address

- **WHEN** the sender is attributed to a cross-relay origin whose origin segment
  is a routable canonical principal id
- **THEN** the `From` header's `session:` token carries the
  `<origin>!<peer-name>` form
- **AND** that token is accepted as a target by cross-relay target resolution

#### Scenario: Render a cross-relay sender whose origin is not routable

- **WHEN** the sender is attributed to a cross-relay origin whose origin segment
  names no routable recipient
- **THEN** the `From` header's `session:` token still carries the
  `<origin>!<peer-name>` form, unaltered
- **AND** the header is not suppressed or rewritten to hide the origin

#### Scenario: Pane header is exempt from bare-canonical emission

- **WHEN** the same `AddressIdentity` feeds both the pane-envelope header and a
  machine-consumed event field
- **THEN** the pane header renders the decorating
  `Display Name <session:session_name>` form via `render_address`
- **AND** the machine event field carries the identity without decoration
