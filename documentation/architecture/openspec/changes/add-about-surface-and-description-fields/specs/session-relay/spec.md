## ADDED Requirements

### Requirement: Bundle and Session Description Fields

Bundle configuration SHALL support optional description metadata fields:

- bundle-level `description`
- session-level `description` on each `[[sessions]]` entry

Description normalization and validation SHALL be:

- trim leading and trailing whitespace before persistence/use
- reject values that are empty after trim with `validation_invalid_description`
- preserve internal newlines
- enforce maximum UTF-8 character length after trim:
  - bundle `description` <= 2048
  - session `description` <= 512

Description fields MAY be omitted.

#### Scenario: Load bundle with valid descriptions

- **WHEN** bundle and session descriptions are within limits after trim
- **THEN** configuration loads successfully

#### Scenario: Reject whitespace-only bundle description

- **WHEN** bundle `description` contains only whitespace characters
- **THEN** runtime rejects configuration with `validation_invalid_description`

#### Scenario: Reject over-length session description

- **WHEN** session `description` exceeds 512 UTF-8 characters after trim
- **THEN** runtime rejects configuration with `validation_invalid_description`

#### Scenario: Preserve internal newlines in description

- **WHEN** a valid description includes internal newline characters
- **THEN** runtime preserves internal newline content in normalized value

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

### Requirement: Relay About Validation and Authorization Order

Relay SHALL evaluate `about` requests in this order:

1. request/bundle/session validation
2. authorization policy evaluation
3. response construction

`about` authorization SHALL reuse capability label `list.read`.

If request is valid/resolved but denied by policy, relay SHALL return
`authorization_forbidden` with canonical denial details schema.

#### Scenario: Validate before authorization for unknown session

- **WHEN** request includes unknown `principal_id`
- **THEN** relay returns `validation_unknown_session`
- **AND** does not return `authorization_forbidden` for that request

#### Scenario: Deny valid about request by policy

- **WHEN** request is valid/resolved
- **AND** policy denies `list.read` for requester
- **THEN** relay returns `authorization_forbidden`
