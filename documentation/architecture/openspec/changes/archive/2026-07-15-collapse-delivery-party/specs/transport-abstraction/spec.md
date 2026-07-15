## MODIFIED Requirements

### Requirement: Structured Delivery Message Payload

`DeliveryEnvelope` SHALL carry structured message data sufficient for every
transport to render its own representation without importing `crate::relay` or
parsing already-rendered text.

The structured payload SHALL include:

- `message_id`,
- message body,
- created timestamp,
- namespace,
- sender, target, and co-recipient identities, each carried as a structured
  `AddressIdentity` (canonical `session@namespace` id plus optional display
  name),
- authenticated sender identity when available,
- choice decider sessions,
- quiescence hints.

The payload SHALL carry each party as an `AddressIdentity` value directly; it
SHALL NOT carry a parallel party type whose canonical id is a bare string
requiring per-transport conversion before rendering. Transports SHALL obtain the
bare canonical id via the non-decorating accessor and the decorating header form
via `render_address`.

The relay SHALL populate these fields after routing and authorization. Transports
SHALL treat attribution fields as read-only input and SHALL NOT infer or rewrite
sender, target, cc, namespace, or authenticated identity. The namespace SHALL be
the routing namespace used in canonical `session@namespace` identifiers and
out-of-band delivery metadata.

#### Scenario: Relay builds structured payload

- **WHEN** the relay worker accepts a delivery task for any target type
- **THEN** it constructs a `DeliveryEnvelope` containing structured message data
  and per-write control hints
- **AND** it does not render pane-envelope text before calling `mailw`

#### Scenario: Payload carries AddressIdentity per party

- **WHEN** the relay constructs the delivery payload
- **THEN** sender, target, and each co-recipient are carried as `AddressIdentity`
  values directly on the payload
- **AND** no transport performs a bare-string-to-identity conversion before
  rendering

#### Scenario: Transport consumes relay-authored attribution

- **WHEN** a transport receives a `DeliveryEnvelope`
- **THEN** it uses the relay-populated sender, target, cc, and authenticated
  identity, and namespace fields as authoritative input
- **AND** it does not derive those fields from transport-local state

#### Scenario: UI and coder transports share payload shape

- **WHEN** the same send request targets UI and coder sessions
- **THEN** the relay constructs payloads from the same structured field set
- **AND** UI renders a stream event while coder transports render pane-envelope
  text from those fields
