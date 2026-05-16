## RENAMED Requirements

- FROM: `### Requirement: UI-Mediated Decision Submitter Gate`
- TO: `### Requirement: Permission Decision Submitter Gate`

## MODIFIED Requirements

### Requirement: Hello Registration Contract

Each client stream SHALL begin with `hello` registration frame containing:

- `bundle_name`
- `session_id`
- `client_class` (`agent` | `ui` | `operator`)

`hello` identity SHALL bind principal/session for that stream using canonical
identity key:

- `(bundle_name, session_id)`

For `client_class=agent`, `session_id` SHALL resolve via bundle
`[[sessions]]` configuration.

For `client_class=ui`, `session_id` SHALL resolve via global TUI sessions from
`<config-root>/tui.toml`.

For `client_class=operator`, `session_id` SHALL resolve via bundle
`[[sessions]]` configuration AND the resolved principal SHALL be authorized
for operator-class registration by the bundle policy preset. Unauthorized
`operator` claims SHALL be rejected with
`validation_invalid_client_class_for_hello`.

If a second stream attempts `hello` for the same identity while the current
owner is still live, relay SHALL reject second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for configured UI session identity

- **WHEN** TUI client sends valid `hello` with `client_class=ui`
- **AND** `session_id` maps to configured global TUI session `id`
- **THEN** relay accepts hello and binds stream owner identity

#### Scenario: Reject hello for unknown UI session identity

- **WHEN** a stream sends `hello` with `client_class=ui`
- **AND** `session_id` is not present in global TUI sessions
- **THEN** relay rejects hello with `validation_unknown_sender`

#### Scenario: Accept hello for authorized operator session identity

- **WHEN** a stream sends `hello` with `client_class=operator`
- **AND** `session_id` maps to a configured bundle session
- **AND** the bundle policy preset authorizes operator-class for that
  principal
- **THEN** relay accepts hello and binds stream owner identity

#### Scenario: Reject hello for unauthorized operator class claim

- **WHEN** a stream sends `hello` with `client_class=operator`
- **AND** the bundle policy preset does not authorize operator class for that
  principal
- **THEN** relay rejects hello with `validation_invalid_client_class_for_hello`

### Requirement: Endpoint Class Routing Behavior

Relay SHALL route recipient delivery by endpoint class:

- `agent` recipients use existing prompt-injection/quiescence delivery path
- `ui` recipients use stream push event delivery path

`operator` is a decision-submitter class only and SHALL NOT be a delivery
recipient in alpha. Inbound message routing to `operator` recipients is
non-operative and reserved for a future class expansion.

For disconnected `ui` recipients, relay SHALL keep pending delivery queued using
existing relay async queue machinery and attempt delivery when same identity
reconnects.

Endpoint class resolution SHALL be deterministic with this precedence:

1. active `hello` registration for target identity
2. otherwise, target configured in bundle with no active registration defaults
   to class `agent`
3. otherwise, target is rejected as unknown

Recipient-class transport matrix SHALL be:

- `agent`: prompt-injection/quiescence path; active stream binding not required
  for routability
- `ui` with active binding: stream push event path
- `ui` without active binding: queue and retry on reconnect
- `operator`: not a delivery target; no routing matrix entry

Non-UI stream-recipient classes (other than `operator`, which is
delivery-inert) are an empty set in MVP.
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

#### Scenario: Reject inbound delivery routed to operator recipient

- **WHEN** routing resolution selects `operator` as endpoint class for an
  inbound message
- **THEN** relay rejects the routing attempt; operator is not a delivery
  recipient in alpha

### Requirement: Permission Decision Submitter Gate

Permission decision actions (`permission.resolve`) SHALL be accepted only from
associated principals whose `client_class ∈ {ui, operator}`.

Operator-class submitters SHALL satisfy the same `grant` policy capability
check as UI-class submitters.

#### Scenario: Reject non-decision-class submitter

- **WHEN** an associated principal with `client_class=agent` submits
  `permission.resolve`
- **THEN** relay rejects with `validation_invalid_client_class_for_action`

#### Scenario: Accept operator-class submitter with grant authorization

- **WHEN** an associated principal with `client_class=operator` submits
  `permission.resolve`
- **AND** the principal has `grant` capability satisfying the policy
- **THEN** relay processes the decision under the same enforcement mapping as
  a `client_class=ui` submitter

#### Scenario: Reject operator submitter without grant authorization

- **WHEN** an associated principal with `client_class=operator` submits
  `permission.resolve`
- **AND** the principal lacks `grant` capability
- **THEN** relay rejects with `authorization_forbidden`
- **AND** denial details include `capability="grant"`

### Requirement: Permission Decision Arbitration

First authorized decision SHALL win across both `ui` and `operator`
submitters. Subsequent decisions on resolved requests SHALL be rejected with
`runtime_permission_request_already_resolved` and SHALL NOT mutate state.

#### Scenario: Reject late decision after prior approval

- **WHEN** a second authorized submitter (ui or operator) decides an already
  resolved request
- **THEN** relay rejects with `runtime_permission_request_already_resolved`

### Requirement: Permission Decision Denial Schema

When relay denies permission decisioning by policy, relay SHALL return
`authorization_forbidden` with canonical minimum details:

- `capability`
- `requester_session`
- `bundle_name`
- `reason`

Optional additive details MAY include `target_session`, `targets`,
`policy_rule_id`, and ACP-specific metadata.

The denial schema applies uniformly to `client_class=ui` and
`client_class=operator` submitters.

#### Scenario: Return canonical denial details for unauthorized decision submitter

- **WHEN** a `{ui, operator}` principal lacks `grant` permission
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include canonical required fields

## ADDED Requirements

### Requirement: Operator Client Class

Relay SHALL recognize `operator` as a stream `client_class` distinct from
`agent` and `ui`.

Operator class is a decision-submitter role in alpha:

- operator-class streams MAY submit `permission.resolve` decisions and
  `permission.list` queries,
- operator-class streams SHALL NOT be inbound delivery targets,
- operator-class streams SHALL NOT receive `permission.snapshot`,
  `permission.requested`, or `permission.resolved` push events; push events
  remain UI-only in alpha.

#### Scenario: Operator class admitted as distinct from agent and ui

- **WHEN** relay enumerates supported stream client classes
- **THEN** the supported set is `{agent, ui, operator}`

### Requirement: Operator-Class Policy Authorization

Bundle policy preset SHALL be the sole source of authority for whether a
configured session may register with `client_class=operator`.

Operator-class authorization SHALL be evaluated at hello time only. Decision
authority (`grant` capability) is evaluated independently per request.

If a configured session attempts `hello` with `client_class=operator` but the
bundle policy preset does not authorize operator class for that principal,
relay SHALL reject with `validation_invalid_client_class_for_hello`.

Operator-class authorization and `grant` capability SHALL remain independent
gates; both must be satisfied for a session to resolve permission requests.

#### Scenario: Reject operator hello when policy preset lacks operator-class authorization

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset for that principal does not authorize
  operator-class registration
- **THEN** relay rejects with `validation_invalid_client_class_for_hello`

#### Scenario: Operator hello accepted without grant capability

- **WHEN** a configured bundle session sends `hello` with
  `client_class=operator`
- **AND** the bundle policy preset authorizes operator-class registration for
  that principal
- **AND** the policy `grant` capability is `none`
- **THEN** relay accepts the hello
- **AND** subsequent `permission.resolve` from that stream is rejected with
  `authorization_forbidden`

### Requirement: Permission List Polling Request

Relay SHALL accept `RelayRequest::PermissionList` from associated principals
with `client_class ∈ {ui, operator}` and `grant` capability satisfying policy.

`PermissionList` returns the current set of pending permission requests for
the requester's bundle.

Same-bundle scope: cross-bundle `PermissionList` attempts SHALL be rejected
with `validation_cross_bundle_unsupported`.

Response payload SHALL include for each pending request the same field set
emitted by `permission.requested` events:

- `message_id`
- `permission_request_id`
- `target_session`
- `requested_kind`
- `requested_details` (including ACP option metadata)
- `enqueued_at`

Response SHALL include a `schema_version` field and a top-level array of
pending records ordered by enqueue `sequence` ascending.

`PermissionList` SHALL NOT mutate queue state.

Push events (`permission.snapshot`, `permission.requested`,
`permission.resolved`) remain UI-only in alpha. Operator-class visibility is
poll-only via `PermissionList`.

#### Scenario: Operator client lists pending requests

- **WHEN** an operator-class principal with `grant` capability submits
  `PermissionList` scoped to its associated bundle
- **THEN** relay returns pending records in FIFO `sequence` order
- **AND** each record contains the `permission.requested` field set

#### Scenario: Reject permission list from agent class

- **WHEN** a principal with `client_class=agent` submits `PermissionList`
- **THEN** relay rejects with `validation_invalid_client_class_for_action`

#### Scenario: Reject permission list from operator without grant

- **WHEN** an operator-class principal without `grant` capability submits
  `PermissionList`
- **THEN** relay rejects with `authorization_forbidden`
- **AND** denial details include `capability="grant"`

#### Scenario: Reject cross-bundle permission list attempt

- **WHEN** a permission list request targets a bundle other than the
  associated bundle
- **THEN** relay rejects with `validation_cross_bundle_unsupported`
