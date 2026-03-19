## ADDED Requirements

### Requirement: About Command Surface

The CLI SHALL expose read-only runtime introspection commands:

- `agentmux about`
- `agentmux about --session <session-id>`
- `agentmux about --bundle <bundle-id>` (same-bundle MVP constraint applies)

`about` authorization SHALL map to capability `list.read`.

#### Scenario: Show bundle about information

- **WHEN** operator runs `agentmux about`
- **THEN** CLI requests bundle/session description payload for associated bundle

#### Scenario: Show one session about information

- **WHEN** operator runs `agentmux about --session relay`
- **THEN** CLI requests bundle about payload filtered to `session_id=relay`

#### Scenario: Reject cross-bundle selector in MVP

- **WHEN** operator provides `--bundle` different from associated bundle context
- **THEN** CLI returns `validation_cross_bundle_unsupported`

### Requirement: About Command Response Schema

CLI machine-readable `about` output SHALL use this exact schema:

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

#### Scenario: Preserve declaration order in about output

- **WHEN** CLI returns about payload for a bundle
- **THEN** `sessions[]` preserves config declaration order

#### Scenario: Serialize null optional values

- **WHEN** bundle or session description is absent
- **THEN** CLI machine output includes explicit `null` value for that field

### Requirement: About Selector Validation and Error Semantics

Validation SHALL run before authorization for `about` requests.

`about` selector failures SHALL use canonical validation codes:

- `validation_unknown_bundle`
- `validation_unknown_session`
- `validation_cross_bundle_unsupported`

Unknown session selectors SHALL return validation errors and SHALL NOT return
successful empty `sessions[]` payloads.

If request is valid/resolved but denied by policy, CLI SHALL surface
`authorization_forbidden` from relay unchanged.

#### Scenario: Reject unknown session selector

- **WHEN** operator requests `agentmux about --session missing`
- **THEN** CLI returns `validation_unknown_session`
- **AND** does not return successful payload with `sessions=[]`

#### Scenario: Surface authorization denial for about

- **WHEN** relay denies valid about request by policy
- **THEN** CLI returns `authorization_forbidden`
