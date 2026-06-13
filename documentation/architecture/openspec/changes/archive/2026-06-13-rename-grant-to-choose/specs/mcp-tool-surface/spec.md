## REMOVED Requirements

### Requirement: Advertise MCP grant meta-tool

**Reason**: `grant` meta-tool retired. Listing moves to `list.decisions` (a
new command on the existing `list` meta-tool); decisioning moves to a
standalone `choose` tool.
**Migration**: Use `list` with `command="decisions"` to list pending requests;
use `choose` to submit a decision.

### Requirement: MCP grant list request contract

**Reason**: Replaced by "MCP list decisions request contract" below.
**Migration**: Use `list` with `command="decisions"`. Field
`permission_request_id` → `choice_request_id` in response entries.

### Requirement: MCP grant resolve request contract

**Reason**: Replaced by "MCP choose request contract" below.
**Migration**: Use `choose` tool directly. Field
`permission_request_id` → `choice_request_id` in request and response.

### Requirement: MCP grant sender authority

**Reason**: Replaced by "MCP choose sender authority" below.
**Migration**: Semantics unchanged; only the tool changes.

### Requirement: MCP grant relay passthrough taxonomy

**Reason**: Replaced by "MCP choose relay passthrough taxonomy" below with
renamed error codes.
**Migration**: Error codes rename from `runtime_permission_*` to
`runtime_choices_*`; `capability` label changes from `"grant"` to `"choose"`.

## MODIFIED Requirements

### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for relay MVP:

- `list`
- `help`
- `look`
- `send`
- `raww`
- `choose`

The relocked pre-stable MCP surface uses `list.principals` with no
compatibility alias for the prior `list.sessions` shape.

#### Scenario: Advertise relocked list meta-tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list`
- **AND** includes `help`
- **AND** includes `look`
- **AND** includes `send`
- **AND** includes `raww`
- **AND** includes `choose`
- **AND** does not include `list.sessions`
- **AND** does not include `grant`

### Requirement: MCP Help Tool

The system SHALL expose a read-only MCP tool named `help` that returns
tool/command discovery metadata and JSON argument schemas.

`help` SHALL support query modes:

- no query (or `query="agentmux"`) returns namespace-level tool inventory
- `query="list"` returns list meta-tool command catalog
- `query="list.principals"` returns exact `list` command argument schema and
  invoke shape for listing principals
- `query="list.decisions"` returns exact `list` command argument schema and
  invoke shape for listing pending decisions
- `query="send"` or `query="look"` or `query="raww"` returns exact tool
  argument schemas and invoke shapes
- `query="choose"` returns the `choose` tool argument schema and invoke shape

Unknown help queries SHALL fail fast with `validation_invalid_params`.

#### Scenario: Return namespace inventory with no query

- **WHEN** an MCP client calls `help` without `query`
- **THEN** the response includes namespace-level tool inventory
- **AND** includes `list`, `help`, `look`, `send`, `raww`, and `choose`

#### Scenario: Return list meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="list"`
- **THEN** the response lists supported `list` commands
- **AND** includes `list.principals`
- **AND** includes `list.decisions`

#### Scenario: Return list.principals argument schema

- **WHEN** an MCP client calls `help` with `query="list.principals"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="principals"`

#### Scenario: Return list.decisions argument schema

- **WHEN** an MCP client calls `help` with `query="list.decisions"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="decisions"`

#### Scenario: Return choose tool argument schema

- **WHEN** an MCP client calls `help` with `query="choose"`
- **THEN** the response includes JSON argument schema for the `choose` tool
- **AND** includes canonical invoke shape

## ADDED Requirements

### Requirement: MCP list decisions request contract

MCP `list` with `command="decisions"` SHALL return pending ACP choice requests
for the associated bundle.

No `bundle_name` field is accepted; bundle scope is derived from the associated
connection context. No additional positional arguments are accepted; unknown
fields SHALL be rejected with `validation_invalid_params`.

Successful response SHALL include:

- `schema_version`
- `pending_requests[]` ordered by enqueue `sequence` ascending

Each entry in `pending_requests[]` SHALL include:

- `message_id`
- `choice_request_id`
- `target_session`
- `requested_kind`
- `requested_details` (including ACP option metadata)
- `enqueued_at`

These fields mirror the `choices.requested` relay event payload.

#### Scenario: List pending choice requests for associated bundle

- **WHEN** caller invokes `list` with `command="decisions"`
- **AND** the MCP stream principal has `client_class=operator` (or `ui`) and
  `choose` capability
- **THEN** MCP returns `pending_requests[]` ordered by `sequence`
- **AND** each entry contains the required field set

#### Scenario: Reject decisions list from principal without choose capability

- **WHEN** caller invokes `list` with `command="decisions"`
- **AND** the MCP stream principal lacks `choose` capability
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "choose"`

### Requirement: MCP choose request contract

MCP `choose` SHALL submit an ACP-native decision on a pending choice request.

Request argument schema:

- `choice_request_id` (required, non-empty string)
- `outcome` (required, value `selected` or `cancelled`)
- `option_id` (required when `outcome="selected"`, forbidden when
  `outcome="cancelled"`)

No `bundle_name` field is accepted. Bundle scope is derived from the associated
connection context.

The following payload fields SHALL be rejected with `validation_invalid_params`:

- `decided_by`
- `ui_session_id`
- `operator_session_id`
- any other caller-supplied sender-like identity field

Unknown fields SHALL be rejected with `validation_invalid_params`.

Successful response SHALL preserve relay decision payload contract including:

- `schema_version`
- `status`
- `choice_request_id`
- `outcome`
- `decided_by` (relay-derived, association-bound)
- optional `reason_code`, `reason`

#### Scenario: Choose with explicit option id

- **WHEN** caller invokes `choose` with `outcome="selected"` and explicit
  `option_id`
- **AND** the MCP stream principal has `client_class=operator` (or `ui`) and
  `choose` capability
- **THEN** MCP forwards the decision to relay using the supplied `option_id`
- **AND** returns the relay decision response unchanged

#### Scenario: Cancel a pending choice request

- **WHEN** caller invokes `choose` with `outcome="cancelled"` and no `option_id`
- **THEN** MCP forwards the decision to relay
- **AND** returns the relay decision response with cancelled outcome

#### Scenario: Reject selected without option_id

- **WHEN** caller invokes `choose` with `outcome="selected"` and missing
  `option_id`
- **THEN** MCP rejects with `validation_invalid_params`

#### Scenario: Reject cancelled with option_id

- **WHEN** caller invokes `choose` with `outcome="cancelled"` and any
  `option_id` value
- **THEN** MCP rejects with `validation_invalid_params`

#### Scenario: Reject payload-supplied sender identity field

- **WHEN** caller invokes `choose` with payload containing `decided_by`,
  `ui_session_id`, or `operator_session_id`
- **THEN** MCP rejects with `validation_invalid_params`

### Requirement: MCP choose sender authority

MCP choice decision sender identity SHALL be association-derived from the MCP
server stream registration context and SHALL NOT be caller-overridable.

`decided_by` in the relay decision response is relay-derived from the
associated principal session id; MCP SHALL pass this field through unchanged
and SHALL NOT mint or transform actor identity.

#### Scenario: Use association-derived sender for choice decisions

- **WHEN** caller invokes MCP `choose`
- **THEN** MCP resolves sender principal from associated session context
- **AND** uses that principal for relay authorization/evaluation
- **AND** echoes relay `decided_by` unchanged in the response

### Requirement: MCP choose relay passthrough taxonomy

MCP `list decisions` and `choose` SHALL preserve canonical relay codes and
payload semantics for validation, authorization, and runtime failures,
including:

- `validation_invalid_params`
- `authorization_forbidden`
- `runtime_choices_request_already_resolved`
- `runtime_choices_queue_full`
- `runtime_choices_queue_unavailable`

For denied `choose` requests, denial details SHALL preserve
`capability = "choose"`.

#### Scenario: Preserve choice denial capability label

- **WHEN** relay denies `choose` by policy
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "choose"`

#### Scenario: Preserve already-resolved code

- **WHEN** relay rejects `choose` because the target request was already
  resolved
- **THEN** MCP returns `runtime_choices_request_already_resolved` unchanged

#### Scenario: Preserve queue-unavailable code

- **WHEN** relay rejects a `list decisions` or `choose` request because the
  persisted queue state is unavailable
- **THEN** MCP returns `runtime_choices_queue_unavailable` unchanged
