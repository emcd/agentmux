## MODIFIED Requirements

### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for relay MVP:

- `list`
- `help`
- `look`
- `send`
- `raww`
- `grant`

The relocked pre-stable MCP surface removes `list.sessions` with no
compatibility alias.

#### Scenario: Advertise relocked list meta-tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list`
- **AND** includes `help`
- **AND** includes `look`
- **AND** includes `send`
- **AND** includes `raww`
- **AND** includes `grant`
- **AND** does not include `list.sessions`

### Requirement: MCP Help Tool

The system SHALL expose a read-only MCP tool named `help` that returns
tool/command discovery metadata and JSON argument schemas.

`help` SHALL support query modes:

- no query (or `query="agentmux"`) returns namespace-level tool inventory
- `query="list"` returns list meta-tool command catalog
- `query="list.sessions"` returns exact `list` command argument schema and
  invoke shape
- `query="send"` or `query="look"` or `query="raww"` returns exact tool
  argument schemas and invoke shapes
- `query="grant"` returns grant meta-tool command catalog
- `query="grant.list"` returns exact `grant` command argument
  schema and invoke shape for listing pending requests
- `query="grant.resolve"` returns exact `grant` command argument
  schema and invoke shape for submitting decisions

Unknown help queries SHALL fail fast with `validation_invalid_params`.

#### Scenario: Return namespace inventory with no query

- **WHEN** an MCP client calls `help` without `query`
- **THEN** the response includes namespace-level tool inventory
- **AND** includes `list`, `help`, `look`, `send`, `raww`, and `grant`

#### Scenario: Return list meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="list"`
- **THEN** the response lists supported `list` commands
- **AND** includes `list.sessions`

#### Scenario: Return list.sessions argument schema

- **WHEN** an MCP client calls `help` with `query="list.sessions"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="sessions"`

#### Scenario: Return grant meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="grant"`
- **THEN** the response lists supported `grant` commands
- **AND** includes `grant.list`
- **AND** includes `grant.resolve`

#### Scenario: Return grant.list argument schema

- **WHEN** an MCP client calls `help` with `query="grant.list"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `grant`
- **AND** includes `command="list"`

#### Scenario: Return grant.resolve argument schema

- **WHEN** an MCP client calls `help` with `query="grant.resolve"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `grant`
- **AND** includes `command="resolve"`

#### Scenario: Reject unknown help query

- **WHEN** an MCP client calls `help` with unknown `query`
- **THEN** MCP returns `validation_invalid_params`

## ADDED Requirements

### Requirement: Advertise MCP grant meta-tool

MCP tool inventory SHALL advertise top-level tool `grant` for ACP
permission queue visibility and operator decisioning.

`grant` SHALL accept a `command` selector with supported values
`list` and `resolve`. Unknown `command` values SHALL be rejected with
`validation_invalid_params`.

#### Scenario: Include grant in tool inventory

- **WHEN** MCP client requests tool catalog
- **THEN** catalog includes `grant`

#### Scenario: Reject grant with unknown command selector

- **WHEN** MCP client calls `grant` with `command` not in
  `{list, resolve}`
- **THEN** MCP rejects with `validation_invalid_params`

### Requirement: MCP grant list request contract

MCP `grant` with `command="list"` SHALL return pending ACP permission
requests for the associated bundle.

Request argument schema:

- `bundle_name` (optional; when present MUST equal the associated bundle, else
  reject with `validation_cross_bundle_unsupported`)

No additional positional arguments are accepted; unknown fields SHALL be
rejected with `validation_invalid_params`.

Successful response SHALL include:

- `schema_version`
- `bundle_name`
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

#### Scenario: Reject grant list with mismatched bundle_name

- **WHEN** caller invokes `grant list` with `bundle_name` other than the
  associated bundle
- **THEN** MCP rejects with `validation_cross_bundle_unsupported`

### Requirement: MCP grant resolve request contract

MCP `grant` with `command="resolve"` SHALL submit an ACP-native decision
on a pending permission request.

Request argument schema:

- `permission_request_id` (required, non-empty string)
- `outcome` (required, value `selected` or `cancelled`)
- `option_id` (required when `outcome="selected"`, forbidden when
  `outcome="cancelled"`)
- `bundle_name` (optional; when present MUST equal the associated bundle, else
  reject with `validation_cross_bundle_unsupported`)

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

### Requirement: MCP grant sender authority

MCP permission decision sender identity SHALL be association-derived from the
MCP server stream registration context and SHALL NOT be caller-overridable.

`decided_by` in the relay decision response is relay-derived from the
associated principal session id; MCP SHALL pass this field through unchanged
and SHALL NOT mint or transform actor identity.

#### Scenario: Use association-derived sender for permission decisions

- **WHEN** caller invokes MCP `grant resolve`
- **THEN** MCP resolves sender principal from associated session context
- **AND** uses that principal for relay authorization/evaluation
- **AND** echoes relay `decided_by` unchanged in the response

### Requirement: MCP grant relay passthrough taxonomy

MCP `grant` SHALL preserve canonical relay codes and payload semantics
for validation, authorization, and runtime failures, including:

- `validation_invalid_params`
- `validation_cross_bundle_unsupported`
- `validation_invalid_client_class_for_action`
- `validation_invalid_client_class_for_hello` (surfaced at relay
  registration; reported via inscription rather than tool response)
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
