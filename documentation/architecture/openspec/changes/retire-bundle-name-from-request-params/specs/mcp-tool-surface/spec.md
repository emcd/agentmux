## MODIFIED Requirements

### Requirement: MCP grant list request contract

MCP `grant` with `command="list"` SHALL return pending ACP permission
requests for the associated bundle.

Request argument schema:

- No `bundle_name` field is accepted. Bundle scope is derived from the
  associated connection context.

No additional positional arguments are accepted; unknown fields SHALL be
rejected with `validation_invalid_params`.

Successful response SHALL include:

- `schema_version`
- `pending_requests[]` ordered by enqueue `sequence` ascending

Each entry in `pending_requests[]` SHALL include:

- `message_id`
- `permission_request_id`
- `target_session`
- `requested_kind`
- `requested_details` (including ACP option metadata)
- `enqueued_at`

These fields mirror the `permission.requested` relay event payload.

#### Scenario: List pending permission requests for associated bundle

- **WHEN** caller invokes `grant` with `command="list"`
- **AND** the MCP stream principal has `client_class=operator` (or `ui`) and
  `grant` capability
- **THEN** MCP returns `pending_requests[]` ordered by `sequence`
- **AND** each entry contains the required field set

### Requirement: MCP grant resolve request contract

MCP `grant` with `command="resolve"` SHALL submit an ACP-native decision
on a pending permission request.

Request argument schema:

- `permission_request_id` (required, non-empty string)
- `outcome` (required, value `selected` or `cancelled`)
- `option_id` (required when `outcome="selected"`, forbidden when
  `outcome="cancelled"`)

No `bundle_name` field is accepted. Bundle scope is derived from the
associated connection context.

The following MCP `grant resolve` payload fields SHALL be rejected with
`validation_invalid_params`:

- `decided_by`
- `ui_session_id`
- `operator_session_id`
- any other caller-supplied sender-like identity field

Unknown fields SHALL be rejected with `validation_invalid_params`.

Successful response SHALL preserve relay decision payload contract including:

- `schema_version`
- `status`
- `permission_request_id`
- `outcome`
- `decided_by` (relay-derived, association-bound)
- optional `reason_code`, `reason`

#### Scenario: Resolve permission with explicit option id

- **WHEN** caller invokes `grant resolve` with
  `outcome="selected"` and explicit `option_id`
- **AND** the MCP stream principal has `client_class=operator` (or `ui`) and
  `grant` capability
- **THEN** MCP forwards the decision to relay using the supplied `option_id`
- **AND** returns the relay decision response unchanged

#### Scenario: Cancel pending permission request

- **WHEN** caller invokes `grant resolve` with `outcome="cancelled"`
  and no `option_id`
- **THEN** MCP forwards the decision to relay
- **AND** returns the relay decision response with cancelled outcome

#### Scenario: Reject selected without option_id

- **WHEN** caller invokes `grant resolve` with `outcome="selected"` and
  missing `option_id`
- **THEN** MCP rejects with `validation_invalid_params`

#### Scenario: Reject cancelled with option_id

- **WHEN** caller invokes `grant resolve` with `outcome="cancelled"` and
  any `option_id` value
- **THEN** MCP rejects with `validation_invalid_params`

#### Scenario: Reject payload-supplied sender identity field

- **WHEN** caller invokes `grant resolve` with payload containing
  `decided_by`, `ui_session_id`, or `operator_session_id`
- **THEN** MCP rejects with `validation_invalid_params`

### Requirement: MCP grant relay passthrough taxonomy

MCP `grant` SHALL preserve canonical relay codes and payload semantics
for validation, authorization, and runtime failures, including:

- `validation_invalid_params`
- `authorization_forbidden`
- `runtime_permission_request_already_resolved`
- `runtime_permission_queue_full`
- `runtime_permission_queue_unavailable`

For denied `grant resolve` requests, denial details SHALL preserve
`capability = "grant"`.

#### Scenario: Preserve permission denial capability label

- **WHEN** relay denies `grant resolve` by policy
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "grant"`

#### Scenario: Preserve already-resolved code

- **WHEN** relay rejects a `grant resolve` because the target request was
  already resolved
- **THEN** MCP returns `runtime_permission_request_already_resolved` unchanged

#### Scenario: Preserve queue-unavailable code

- **WHEN** relay rejects a `grant` request because the persisted queue
  state is unavailable
- **THEN** MCP returns `runtime_permission_queue_unavailable` unchanged

