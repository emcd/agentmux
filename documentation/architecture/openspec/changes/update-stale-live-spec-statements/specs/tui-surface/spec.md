## MODIFIED Requirements

### Requirement: Initial TUI Workflow Coverage

The system SHALL define an initial TUI that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery-events inspection and pending indicator.

The TUI SHALL reuse existing relay delivery and inspection semantics.
The TUI SHALL consume inbound message and delivery-outcome updates from the
relay stream transport contracts specified by the `look-and-stream-events`
capability's `Relay Stream Event Contract` and `Hello Registration Contract`
requirements.

#### Scenario: Cover core operator loop with structured update flow

- **WHEN** an operator uses the TUI for routine coordination
- **THEN** the TUI supports recipient selection, send, look, and update
  workflows
- **AND** inbound message and delivery outcome updates are represented using
  canonical relay stream event payloads
