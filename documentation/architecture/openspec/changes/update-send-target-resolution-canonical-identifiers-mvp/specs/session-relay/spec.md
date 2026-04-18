## MODIFIED Requirements

### Requirement: Session Routing Primitive

The system SHALL expose session ids as the routing primitive for message
delivery.
The system SHALL resolve each target session to its currently active pane at
delivery time.
The system SHALL support directed delivery to one or more explicitly selected
target sessions.

For send explicit targets, relay SHALL accept only canonical target
identifiers:

- configured bundle member `session_id`,
- configured/registered UI session id (when UI routing is supported).

Relay SHALL NOT resolve configured bundle session `name` values as send-target
aliases.
Session `name` remains informational metadata only and is not send-routable.

When one explicit token exactly matches both a bundle member `session_id` and a
UI session id, relay SHALL route to the bundle member `session_id`.

#### Scenario: Resolve session target for direct send

- **WHEN** a caller sends a message to one target session id
- **THEN** the system routes by that session id
- **AND** resolves the session's active pane as the concrete tmux injection
  endpoint

#### Scenario: Reject configured name alias as explicit send target

- **WHEN** a caller sends a message using a configured session `name` token
- **THEN** relay rejects the target with `validation_unknown_target`

#### Scenario: Prefer bundle member session_id on overlap with UI session id

- **WHEN** an explicit token matches both bundle member `session_id` and UI
  session id
- **THEN** relay routes to the bundle member target

### Requirement: Authorization Evaluation Order

Relay SHALL evaluate requests in this order:

1. request validation
2. requester identity resolution
3. bundle/target/action resolution
4. authorization policy evaluation
5. execution

Validation failures SHALL take precedence over authorization denials.

Unknown/non-canonical explicit target tokens SHALL use
`validation_unknown_target`.

#### Scenario: Prefer validation failure over authorization denial for non-send target

- **WHEN** a non-send request includes an unknown target session
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

#### Scenario: Prefer send explicit-target validation over authorization denial

- **WHEN** a send request includes an unknown or non-canonical explicit target
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request
