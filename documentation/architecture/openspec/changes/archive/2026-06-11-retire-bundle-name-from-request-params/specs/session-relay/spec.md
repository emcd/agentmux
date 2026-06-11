## MODIFIED Requirements

### Requirement: Same-Bundle Permission Decision Scope

Permission request routing and decisioning SHALL be same-bundle only in alpha.
The bundle scope SHALL be derived from the request's routing namespace
(frame-level namespace, defaulting to the connection's bound bundle); no
caller-supplied in-payload bundle selector is accepted in `PermissionResolve`.
A caller reaches a bundle's permission queue only by routing to that bundle,
subject to its policy controls.

#### Scenario: Permission decision scoped to session's associated bundle

- **WHEN** a grant-authorized principal issues `PermissionResolve`
- **THEN** relay resolves the permission request within the principal's
  associated bundle
- **AND** no `bundle_name` field is accepted in the request payload

### Requirement: Permission List Polling Request

Relay SHALL accept `RelayRequest::PermissionList` from associated principals
with `client_class ∈ {ui, operator}` and `grant` capability satisfying policy.

`PermissionList` returns the current set of pending permission requests for
the requester's bundle. The bundle scope is derived from the request's routing namespace; no
caller-supplied bundle selector is accepted.

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
  `PermissionList`
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

## REMOVED Requirements

### Requirement: Same-Bundle Stream Scope Enforcement

**Reason**: This requirement mandated that the relay reject any request frame
whose target falls outside the stream's bound bundle with
`validation_cross_bundle_unsupported`. Cross-bundle routing now operates via
the relay routing layer (canonical `session@bundle` ids); same-bundle
enforcement at the frame level is superseded. The
`validation_cross_bundle_unsupported` code is unreachable after this change
completes the `bundle_name` retirement on the request side.
