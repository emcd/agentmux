## ADDED Requirements

### Requirement: Relay About Operation

Relay SHALL provide a read-only operation named `about`.

`about` request fields SHALL include:

- `requester_session` (required)
- `principal_id` (optional)

`about` scope SHALL remain same-bundle only. Bundle scope is derived
from the request's routing namespace (frame-level namespace, defaulting to
the connection's bound bundle); no in-payload bundle selector is accepted.

#### Scenario: Resolve associated bundle from routing context

- **WHEN** request is received on a bundle-bound stream
- **THEN** relay resolves bundle from the routing namespace of that stream


### Requirement: Relay About Response Contract

Successful relay `about` responses SHALL include exactly:

- `schema_version` (string)
- `bundle_name` (string)
- `bundle_description` (string|null)
- `sessions` (array)

Each `sessions` entry SHALL include exactly:

- `session_id` (string)
- `session_name` (string|null)
- `description` (string|null)

`sessions` SHALL preserve bundle configuration declaration order.

Optional fields SHALL serialize as explicit null values and SHALL NOT be omitted.

If request provides `principal_id`, response SHALL contain exactly one matching
entry in `sessions[]`.

Unknown session selectors SHALL return `validation_unknown_session` and SHALL
NOT return successful empty `sessions[]` payloads.

#### Scenario: Return bundle-level about payload

- **WHEN** request omits `principal_id`
- **THEN** relay returns all configured sessions in declaration order

#### Scenario: Return one session for valid session selector

- **WHEN** request includes known `principal_id`
- **THEN** relay returns exactly one entry in `sessions[]`

#### Scenario: Reject unknown session selector

- **WHEN** request includes unknown `principal_id`
- **THEN** relay returns `validation_unknown_session`
- **AND** does not return successful payload with `sessions=[]`
