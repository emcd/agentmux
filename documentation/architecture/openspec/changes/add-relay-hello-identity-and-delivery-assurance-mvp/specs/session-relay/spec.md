## MODIFIED Requirements

### Requirement: Hello Registration Contract

Each client stream SHALL begin with `hello` registration frame containing:

- `schema_version`
- `bundle_name`
- `session_id`
- `client_class` (`agent` | `ui`)

`hello` identity SHALL bind principal/session for that stream using canonical
tuple `(bundle_name, session_id)`.

`session_id` SHALL resolve to a configured bundle session id for the associated
bundle. Display labels or aliases are non-authoritative for stream identity.

Relay SHALL maintain at most one active owner binding for each
`(bundle_name, session_id)` identity.

If a second stream attempts `hello` for the same identity while the current
owner is still live, relay SHALL reject the claim with
`runtime_identity_claim_conflict`.

`runtime_identity_claim_conflict` details SHALL include required fields:

- `bundle_name`
- `session_id`
- `reason`

Conflict details MAY include optional debug fields, including:

- `existing_connection_id`
- `existing_owner_token`

Relay MAY replace ownership for the same identity in MVP only when hard-dead
evidence already exists for the current owner binding:

- observed stream close,
- read/write failure, or
- explicit disconnect event already observed by relay.

MVP claim handling SHALL NOT run active liveness probes in the claim path.

If `hello.bundle_name` does not match relay's associated bundle context, relay
SHALL reject `hello` with `validation_cross_bundle_unsupported`.

If `hello.session_id` is not a configured session id in the associated bundle,
relay SHALL reject `hello` with `validation_unknown_sender`.

#### Scenario: Register agent-class stream

- **WHEN** MCP client sends valid `hello` with `client_class=agent`
- **THEN** relay registers live agent endpoint for that identity

#### Scenario: Register ui-class stream

- **WHEN** TUI client sends valid `hello` with `client_class=ui`
- **THEN** relay registers live UI endpoint for that identity

#### Scenario: Reject duplicate live claim for same identity

- **WHEN** one stream already owns a live `(bundle_name, session_id)` binding
- **AND** a second stream sends `hello` for that same identity
- **THEN** relay rejects second claim with `runtime_identity_claim_conflict`

#### Scenario: Replace identity owner only after hard-dead evidence

- **WHEN** prior owner binding has hard-dead evidence already observed by relay
- **AND** a new stream sends `hello` for the same identity
- **THEN** relay accepts the new binding
- **AND** does not run claim-path liveness probes

#### Scenario: Reject hello for mismatched bundle context

- **WHEN** a stream sends `hello` with `bundle_name` not matching associated
  relay bundle context
- **THEN** relay rejects with `validation_cross_bundle_unsupported`

#### Scenario: Reject hello for unknown session id

- **WHEN** a stream sends `hello` with `session_id` not configured in
  associated bundle
- **THEN** relay rejects with `validation_unknown_sender`

### Requirement: Endpoint Class Routing Behavior

Relay SHALL route recipient delivery by endpoint class:

- `agent` recipients use existing prompt-injection/quiescence delivery path
- `ui` recipients use stream push event delivery path

For disconnected `ui` recipients, relay SHALL keep pending delivery queued using
existing relay async queue machinery and attempt delivery when same identity
reconnects.

Endpoint class resolution SHALL be deterministic with this precedence:

1. active `hello` registration for target identity
2. otherwise, target configured in bundle with no active registration defaults
   to class `agent`
3. otherwise, target is rejected as unknown

Recipient-class transport matrix in MVP SHALL be:

- `agent`: prompt-injection/quiescence path; active stream binding not required
  for routability
- `ui` with active binding: stream push event path
- `ui` without active binding: queue and retry on reconnect

Non-UI stream-recipient classes are an empty set in MVP.
Therefore, no-live-binding fail-fast rules for non-UI stream recipients are
non-operative in MVP and reserved for a future class expansion.

#### Scenario: Deliver to agent recipient via prompt injection path

- **WHEN** target recipient is class `agent`
- **THEN** relay uses existing prompt-injection/quiescence delivery behavior

#### Scenario: Deliver to connected ui recipient via stream event

- **WHEN** target recipient is class `ui`
- **AND** recipient has active registered stream
- **THEN** relay emits inbound-message event frame to that stream

#### Scenario: Queue ui delivery while stream is disconnected

- **WHEN** target recipient is class `ui`
- **AND** recipient has no active registered stream
- **THEN** relay keeps pending delivery queued
- **AND** attempts delivery when same identity reconnects

#### Scenario: Default unregistered configured recipient to agent class

- **WHEN** target recipient is configured in bundle
- **AND** target has no active registration
- **THEN** relay resolves endpoint class as `agent`

#### Scenario: Reject unregistered unknown recipient

- **WHEN** target has no active registration
- **AND** target is not configured in associated bundle
- **THEN** relay rejects request with `validation_unknown_recipient`

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `bundle_name`
- `target_session`
- `created_at`

MVP event types SHALL include:

- `incoming_message`
- `delivery_outcome`

`incoming_message` payload SHALL include:

- `message_id`
- `sender_session`
- `body`
- optional `cc_sessions`

`delivery_outcome` payload SHALL include:

- `message_id`
- `phase` (`routed`|`delivered`|`failed`)
- `outcome` (`success`|`timeout`|`failed`|null)
- optional `reason_code`
- optional `reason`

`delivery_outcome` SHALL be the canonical machine completion/update carrier for
stream-path delivery updates and SHALL be keyed by `message_id`.

`phase=routed` SHALL be diagnostic metadata and SHALL set `outcome=null`.

Terminal updates SHALL keep existing external vocabulary:

- delivered terminal: `phase=delivered`, `outcome=success`
- failure terminal: `phase=failed`, `outcome` in (`timeout`|`failed`)

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `phase=failed`
- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Push routed diagnostic update

- **WHEN** relay resolves stream routing for a target delivery
- **THEN** relay pushes `delivery_outcome` with `phase=routed`
- **AND** sets `outcome=null`

#### Scenario: Push terminal delivery outcome update

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical `phase` and `outcome` values

#### Scenario: Map dropped_on_shutdown to failed terminal update

- **WHEN** relay terminal state for a target is `dropped_on_shutdown`
- **THEN** `delivery_outcome` includes `phase=failed`
- **AND** includes `outcome=failed`
- **AND** includes `reason_code=dropped_on_shutdown`
