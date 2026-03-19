## ADDED Requirements

### Requirement: MCP About Tool

The system SHALL expose a read-only MCP tool named `about`.

`about` request parameters SHALL be:

- `session_id` (optional)
- `bundle_name` (optional; redundant under associated bundle context)

`about` authorization SHALL map to capability `list.read`.

#### Scenario: Advertise about tool

- **WHEN** MCP client enumerates tools
- **THEN** MCP tool inventory includes `about`

#### Scenario: Query associated bundle about payload

- **WHEN** caller invokes `about` without selectors
- **THEN** MCP resolves associated bundle context and returns about payload

#### Scenario: Reject cross-bundle about selector in MVP

- **WHEN** caller provides `bundle_name` different from associated bundle
- **THEN** MCP returns `validation_cross_bundle_unsupported`

### Requirement: MCP About Response Contract

Successful `about` tool responses SHALL include exactly:

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

#### Scenario: Return one session when session selector is provided

- **WHEN** caller invokes `about` with `session_id`
- **THEN** response contains exactly one matching session entry in `sessions[]`

#### Scenario: Preserve null optional fields in MCP response

- **WHEN** description values are absent
- **THEN** MCP response includes explicit null fields

### Requirement: MCP About Validation and Authorization Semantics

Validation SHALL run before authorization for `about` requests.

`about` selector validation failures SHALL use:

- `validation_unknown_bundle`
- `validation_unknown_session`
- `validation_cross_bundle_unsupported`

Unknown session selectors SHALL return validation errors and SHALL NOT return
successful empty `sessions[]` payloads.

If request is valid/resolved but denied by policy, MCP SHALL return
`authorization_forbidden` using the existing canonical denial details schema.

#### Scenario: Reject unknown session id

- **WHEN** caller invokes `about` with `session_id` not in bundle
- **THEN** MCP returns `validation_unknown_session`

#### Scenario: Return canonical authorization denial for about

- **WHEN** request is valid/resolved but denied by policy
- **THEN** MCP returns `authorization_forbidden`
