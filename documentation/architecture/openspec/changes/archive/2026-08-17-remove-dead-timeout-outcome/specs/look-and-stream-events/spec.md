## MODIFIED Requirements

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `target_session`
- `created_at`

`target_session` SHALL carry the canonical `session@bundle` form per the
Canonical Session Identity requirement. `bundle_name` is retired; bundle
context is recoverable from the `target_session` suffix.

Event types SHALL include:

- `incoming_message`
- `delivery_outcome`

`incoming_message` payload SHALL include:

- `message_id`
- `sender_session`
- `body`
- optional `cc_sessions`

`sender_session` and `cc_sessions` SHALL carry the bare canonical
`session@namespace` form obtained via the non-decorating identity accessor.
They SHALL NOT carry the decorating pane-header form
(`Display Name <session:session_name>`) produced by `render_address`. The
pane-envelope From/To/Cc header is the only surface that uses the decorating
form; the `incoming_message` machine event fields are exempt from it.

`delivery_outcome` payload SHALL include:

- `message_id`
- `phase` (`routed`|`delivered`|`failed`|`not_submitted`|`submission_unknown`)
- `outcome` (`success`|`failed`|`not_submitted`|`submission_unknown`|null)
- optional `reason_code`
- optional `reason`

`delivery_outcome` SHALL be the canonical machine completion/update carrier for
stream-path delivery updates and SHALL be keyed by `message_id`.

`phase=routed` SHALL be diagnostic metadata and SHALL set `outcome=null`.

Terminal updates SHALL keep existing external vocabulary:

- delivered terminal: `phase=delivered`, `outcome=success`
- failure terminal: `phase=failed`, `outcome=failed`
- provable non-delivery terminal: `phase=not_submitted`,
  `outcome=not_submitted`
- indeterminate-submission terminal: `phase=submission_unknown`,
  `outcome=submission_unknown`

`not_submitted` and `submission_unknown` SHALL each carry their own `phase` and
`outcome` spelling rather than being reported as a failure terminal. They make
opposite evidentiary claims — one asserts that no target-side effect occurred,
the other that such an effect cannot be excluded — and collapsing either into
`failed` would assert a non-delivery the relay cannot support.

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `phase=failed`
- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Emit bare canonical sender and cc identity in incoming_message

- **WHEN** the sender identity is `session_name = "alice@bundle"` with
  `display_name = "Alice Cooper"` and a co-recipient has the same shape
- **THEN** the `incoming_message` event `sender_session` equals `"alice@bundle"`
- **AND** each entry in `cc_sessions` is the bare canonical `session@namespace`
  id
- **AND** neither field carries the decorating
  `Display Name <session:session_name>` form

#### Scenario: Push routed diagnostic update

- **WHEN** relay resolves stream routing for a target delivery
- **THEN** relay pushes `delivery_outcome` with `phase=routed`
- **AND** sets `outcome=null`

#### Scenario: Push terminal delivery outcome update

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical `phase` and `outcome` values

#### Scenario: Emit an evidence-bearing terminal outcome under its own spelling

- **WHEN** relay records a terminal delivery outcome of `not_submitted` or
  `submission_unknown` for a message target
- **THEN** `delivery_outcome` carries that spelling as both `phase` and
  `outcome`
- **AND** does not report it as `phase=failed`

#### Scenario: Map dropped_on_shutdown to failed terminal update

- **WHEN** relay terminal state for a target is `dropped_on_shutdown`
- **THEN** `delivery_outcome` includes `phase=failed`
- **AND** includes `outcome=failed`
- **AND** includes `reason_code=dropped_on_shutdown`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`
