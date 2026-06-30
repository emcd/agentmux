## MODIFIED Requirements

### Requirement: Address Identity Format

Header addresses SHALL support display names and canonical session identifiers
using:

- `Display Name <session:session@namespace>`

The `session:` identity token SHALL carry the canonical `session@namespace`
principal id, so a recipient in any namespace can derive a reply address from
the envelope alone. When an address has no configured display name in the
delivery bundle (for example a co-recipient in another namespace), the display
portion SHALL fall back to the canonical id.

This decorating form is REQUIRED for the pane-envelope From/To/Cc header and is
the only delivery surface that uses it. The pane header is EXEMPT from the
bare-canonical-identity rule that governs machine-consumed event fields (for
example the relay `incoming_message` event's `sender_session` / `cc_sessions`),
which carry the bare canonical `session@namespace` id without decoration.

#### Scenario: Render sender with display name

- **WHEN** sender display metadata is available
- **THEN** `From` header includes display name and `session:` identity token
- **AND** the identity token carries the canonical `session@namespace` id

#### Scenario: Render cross-namespace address without configured display name

- **WHEN** an address has no configured display name in the delivery bundle
- **THEN** the address renders the canonical id in both the display and
  identity positions

#### Scenario: Pane header is exempt from bare-canonical emission

- **WHEN** the same `AddressIdentity` feeds both the pane-envelope header and a
  machine-consumed event field
- **THEN** the pane header renders the decorating
  `Display Name <session:session@namespace>` form via `render_address`
- **AND** the machine event field carries the bare canonical
  `session@namespace` id
