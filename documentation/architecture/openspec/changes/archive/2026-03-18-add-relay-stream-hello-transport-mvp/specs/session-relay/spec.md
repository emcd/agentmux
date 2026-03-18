## ADDED Requirements

### Requirement: Persistent Relay Client Streams

Relay SHALL support long-lived full-duplex Unix socket client streams.

Client request/response frames and relay-pushed event frames SHALL share the
same stream connection.

Relay SHALL reject protocol frames received before successful `hello`
registration with `validation_missing_hello`.

#### Scenario: Accept request/response exchange on persistent stream

- **WHEN** a client opens relay stream and completes `hello`
- **THEN** client can send request frames on that stream
- **AND** relay returns response frames on that same stream without closing it

#### Scenario: Reject request before hello

- **WHEN** client sends request frame before successful `hello`
- **THEN** relay rejects frame with `validation_missing_hello`

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

If a second stream registers the same `(bundle_name, session_id)`, relay
SHALL replace the prior live binding with latest successful hello.

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

#### Scenario: Replace prior stream on identity reconnect

- **WHEN** a new stream successfully `hello`-registers same identity
- **THEN** relay invalidates prior live stream binding
- **AND** uses latest stream as authoritative live endpoint

#### Scenario: Reject hello for mismatched bundle context

- **WHEN** a stream sends `hello` with `bundle_name` not matching associated
  relay bundle context
- **THEN** relay rejects with `validation_cross_bundle_unsupported`

#### Scenario: Reject hello for unknown session id

- **WHEN** a stream sends `hello` with `session_id` not configured in
  associated bundle
- **THEN** relay rejects with `validation_unknown_sender`

### Requirement: Same-Bundle Stream Scope Enforcement

Persistent stream routing in MVP SHALL remain same-bundle only.

Relay SHALL reject cross-bundle stream/request attempts with
`validation_cross_bundle_unsupported`.

#### Scenario: Reject cross-bundle request frame

- **WHEN** a registered stream submits request frame scoped to bundle that does
  not match stream identity bundle
- **THEN** relay rejects with `validation_cross_bundle_unsupported`

### Requirement: Static Recipient Routability

Static configured recipients from bundle session definitions SHALL remain
routable independent of active stream presence or prior `hello` from those
recipients.

#### Scenario: Route to configured recipient before recipient stream registration

- **WHEN** sender targets a configured bundle recipient
- **AND** recipient has no active stream registration
- **THEN** relay processes routing using configured recipient identity semantics
- **AND** does not reject solely for missing recipient `hello`

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
- `outcome` (`success`|`timeout`|`failed`)
- optional `reason_code`
- optional `reason`

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Push terminal delivery outcome event

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical outcome + reason fields

### Requirement: Stream Failure Semantics

Relay SHALL fail fast on malformed hello/protocol frames.

Relay SHALL surface stream disconnect events through runtime diagnostics and
continue serving other active streams.

#### Scenario: Reject malformed hello payload

- **WHEN** client sends malformed or invalid hello frame
- **THEN** relay rejects with structured validation error
- **AND** does not register stream identity

#### Scenario: Continue serving other streams after one disconnect

- **WHEN** one client stream disconnects unexpectedly
- **THEN** relay records diagnostic event
- **AND** continues serving other active client streams
