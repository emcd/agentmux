# pane-envelope Spec Delta

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

#### Scenario: Render sender with display name

- **WHEN** sender display metadata is available
- **THEN** `From` header includes display name and `session:` identity token
- **AND** the identity token carries the canonical `session@namespace` id

#### Scenario: Render cross-namespace address without configured display name

- **WHEN** an address has no configured display name in the delivery bundle
- **THEN** the address renders the canonical id in both the display and
  identity positions

### Requirement: CC Informational Semantics

`Cc` metadata SHALL be informational and SHALL NOT override canonical routing.

Each delivered envelope's `Cc` header SHALL list every co-recipient of the
message — the full target set minus the envelope's own recipient — including
co-recipients in other namespaces.

#### Scenario: Preserve routing independent of Cc header

- **WHEN** envelope includes `Cc` header values
- **THEN** delivery routing remains derived from relay request targets

#### Scenario: Show cross-namespace co-recipients in Cc

- **WHEN** a message targets recipients in more than one namespace
- **THEN** each delivered envelope's `Cc` lists the co-recipients from every
  other namespace as canonical ids
