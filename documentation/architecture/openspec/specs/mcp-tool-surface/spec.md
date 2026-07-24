# mcp-tool-surface Specification

## Purpose
The MCP tool inventory and per-tool request/response contracts surfaced to MCP clients — `list`, `help`, `look`, `send`, `raww`, `choose`. The spec governs tool set stability (no temporary aliases; `grant` is not exposed after the rename-to-choose archive), canonical error taxonomy passthrough (validation precedes authorization denials with canonical denial details), list namespace selectors (`omitted`/`home-bundle`/`GLOBAL`/`*`), and sender identity inference from MCP association (caller-supplied identity fields are rejected). `relay_unavailable` fallback semantics apply to all tools when the bundle relay is unreachable.
## Requirements
### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools:

- `list`
- `help`
- `look`
- `send`
- `raww`
- `choose`
- `updown`
- `new`
- `change`

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

#### Scenario: Advertise admin meta-tools

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `updown`
- **AND** includes `new`
- **AND** includes `change`

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

### Requirement: Manual Bundle Configuration

The system SHALL treat bundle definitions as operator-managed configuration
and SHALL NOT expose MCP tools that mutate bundle configuration.

#### Scenario: Exclude configuration mutation tools from MCP surface

- **WHEN** an MCP client enumerates available tools
- **THEN** tool list excludes bundle mutation operations

### Requirement: Recipient Listing Contract

`list` with `command="principals"` SHALL return bundle principal listing
payloads.

Successful responses SHALL always use the aggregate shape, regardless of the
`namespace` selector:

- `schema_version`
- `bundles[]` (array of canonical bundle objects)

A relay-wide `GLOBAL` bundle view SHALL always be appended to `bundles[]` as the
final entry, regardless of the requested `namespace` (so a home-bundle or
named-bundle request returns the resolved bundle followed by the `GLOBAL` view;
a `namespace="GLOBAL"` request returns only the `GLOBAL` view).

Each `bundles[]` entry SHALL include:

- `id`
- `state` (`up`|`down`)
- `startup_health` (`healthy`|`degraded`) (required when `state=up`;
  omitted when `state=down`)
- `state_reason_code` (required when `state=down`; omitted when `state=up`)
- `state_reason` (optional)
- `startup_failure_count` (required integer)
- `recent_startup_failures` (required array; may be empty)
- `principals[]`

Each `principals[]` entry SHALL include:

- `id`
- `name` (optional)
- `transport` (`tmux`|`acp`)

Each `recent_startup_failures[]` entry SHALL include:

- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

If requester identity is valid and policy denies relay-handled list access, MCP
SHALL return `authorization_forbidden` and SHALL NOT return a successful list
payload.

#### Scenario: Include startup health and startup-failure fields in successful list payload

- **WHEN** `list` with `command="principals"` succeeds for one bundle
- **THEN** each `bundles[]` entry includes required startup health/state fields
- **AND** includes required startup failure history fields

#### Scenario: Append GLOBAL view regardless of selector

- **WHEN** `list` with `command="principals"` succeeds for a single bundle
- **THEN** the response `bundles[]` includes the resolved bundle
- **AND** appends a final `GLOBAL` bundle view

#### Scenario: Omit startup health for down state

- **WHEN** a `bundles[]` entry state is `down`
- **THEN** that entry omits `startup_health`
- **AND** includes required `state_reason_code`

#### Scenario: Deny list request with authorization_forbidden

- **WHEN** requester identity is valid
- **AND** policy denies list visibility for requester
- **THEN** MCP returns `authorization_forbidden`
- **AND** does not return successful `bundles[]` output

### Requirement: Send Target Selection

Send target identifiers SHALL be:

- bundle member session id
- UI session id (where UI routing is supported)

Configured session `name` values and display-name aliases are not canonical send
target identifiers and SHALL NOT be relay-routed.

If one token matches both a bundle member `session_id` and UI session id, the
bundle member `session_id` interpretation SHALL win.

`send` SHALL NOT accept any transport-scoped timeout override
field in v1. v1 of ACP delivery and v1 of Tmux delivery are fully
config-only: the per-coder config keys
`[coders.<id>.acp].prime-timeout-ms` and
`[coders.<id>.tmux].prime-timeout-ms` are the only timeout
surfaces.

The pre-existing `quiescence_timeout_ms` payload field was
retired by the `tmux-wedge-detection` proposal; the pre-existing
`acp_turn_timeout_ms` payload field is retired by this proposal.
Both fields are rejected by the MCP server as unknown. Alpha
defaults apply: the rejection is a generic unknown-field error;
the server is NOT required to name a replacement. Operators who
hit the rejection consult the changelog.

`send` SHALL reject ACP timeout overrides against non-ACP targets
with `validation_invalid_timeout_field_for_transport`. With no
transport-scoped timeout override fields in v1, this validation
class is reserved for future per-call overrides (if/when a
transport-neutral `prime_timeout_ms` payload field is
reintroduced — see `design.md` Future Work).

`send` authorization scope SHALL follow requester policy control:

- `home`
- `all`

#### Scenario: Reject non-canonical configured-name token for explicit send target

- **WHEN** `send` targets a configured session `name` token
- **THEN** the tool returns `validation_unknown_target`

#### Scenario: Resolve overlap token as bundle member session_id

- **WHEN** one explicit target token matches both bundle member `session_id` and
  UI session id
- **THEN** the token is interpreted as bundle member `session_id`

#### Scenario: Reject retired tmux timeout payload field

- **WHEN** `send` request payload includes
  `quiescence_timeout_ms` (a field that does not exist in v1)
- **THEN** the tool rejects the field as unknown

#### Scenario: Reject retired ACP timeout payload field

- **WHEN** `send` request payload includes
  `acp_turn_timeout_ms` (a field that does not exist in v1)
- **THEN** the tool rejects the field as unknown

#### Scenario: Reject hypothetical ACP prime timeout payload field

- **WHEN** `send` request payload includes
  `acp_prime_timeout_ms` (a field that has never existed)
- **THEN** the tool rejects the field as unknown

### Requirement: Sender Identity Inference

`send` SHALL infer sender identity from the MCP server's configured session
association and SHALL NOT require a sender identity in request payloads.

Association/socket-driven requester identity SHALL be authoritative for
principal identity.
Caller-supplied sender-like payload fields SHALL NOT override that principal.

#### Scenario: Infer sender session identity

- **WHEN** a caller invokes `send`
- **THEN** the system resolves sender identity from MCP server association
- **AND** uses that sender session identity for delivery metadata

#### Scenario: Reject unbound sender identity

- **WHEN** the MCP server instance has no valid session association
- **THEN** the system rejects the request with `validation_unknown_sender`

### Requirement: Send Response Contract

`send` SHALL return a response containing:

- `schema_version`
- `request_id` (when provided by caller)
- `requester_session`
- `sender_display_name` (optional)
- `results` (per-target entries)

`bundle_name` is retired from send responses; bundle context is recoverable
from the `requester_session` suffix.

Each per-target result SHALL include:

- `target_session`
- `message_id`
- `outcome` = `queued`

#### Scenario: Return accepted outcome for send request

- **WHEN** a caller invokes `send`
- **THEN** per-target outcomes are `queued`

#### Scenario: Return empty results for zero effective recipients

- **WHEN** a caller invokes `send`
- **AND** effective target resolution yields zero recipients
- **THEN** the response includes `results=[]`

### Requirement: Error Object Contract

Tool failures SHALL return a structured error object with:

- `code`
- `message`
- `details` (optional object)

The system SHALL use stable machine-readable error codes.

For `authorization_forbidden`, `details` SHALL include:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

Validation failures SHALL be returned before authorization denials.

#### Scenario: Unknown bundle error

- **WHEN** a caller references a bundle that does not exist
- **THEN** the tool returns error code `validation_unknown_bundle`
- **AND** includes a human-readable message

#### Scenario: Unknown target error

- **WHEN** `send` targets a token that is not a canonical send target identifier
- **THEN** the tool returns error code `validation_unknown_target`
- **AND** includes a human-readable message

#### Scenario: Return canonical authorization denial schema

- **WHEN** request is valid/resolved but denied by policy
- **THEN** the tool returns `authorization_forbidden`
- **AND** details include the required denial fields

### Requirement: MCP Schema Versioning

All successful responses for relay tools SHALL include `schema_version`.

#### Scenario: Include schema version in success response

- **WHEN** any relay MCP tool succeeds
- **THEN** the response includes `schema_version`

### Requirement: MCP Inspection Naming Exception

The system SHALL expose inspection through MCP tool name `look`.
This SHALL be treated as an explicit and stable exception to delivery tool
naming, where `send` remains reserved for delivery operations.

#### Scenario: Keep inspection separate from send-family semantics

- **WHEN** an MCP client performs session inspection
- **THEN** the client invokes tool `look`
- **AND** inspection is not modeled as an extension of delivery tool `send`

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier; MAY be a peer-qualified
  `<session>@<bundle>` id to inspect a session in a peer bundle)
- `lines` (optional positive integer)

A bare `target_session` is qualified to the MCP server's bound bundle before the
relay call; a target that already carries an `@<bundle>` suffix is forwarded
verbatim (so a peer-bundle target reaches that bundle). A bare target on a
relay-wide (unassociated) MCP server is rejected with
`validation_unqualified_target`. Cross-bundle resolution and authorization are
performed by the relay and surfaced unchanged.

#### Scenario: Advertise look tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `look`

#### Scenario: Reject invalid lines in look request

- **WHEN** a caller provides `lines` outside valid range
- **THEN** the tool returns `validation_invalid_lines`

#### Scenario: Inspect peer bundle session via qualified target

- **WHEN** a caller provides `target_session = "<session>@<peer-bundle>"`
  naming a bundle other than the associated bundle
- **THEN** the tool forwards the target and returns the relay's peer-bundle
  snapshot when the requester is authorized at `look = all`

#### Scenario: Reject unknown bundle

- **WHEN** the target names a bundle that is not configured on the relay
- **THEN** the tool returns `validation_unknown_bundle`

#### Scenario: Reject unknown target

- **WHEN** caller requests inspection for a session that is not a member of the
  resolved bundle
- **THEN** tool returns `validation_unknown_target`

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `structured_entries_v1`)

`bundle_name` is retired from look responses; bundle context is recoverable
from the `target_session` suffix.

When `snapshot_format = "lines"`, MCP responses SHALL include:
- `snapshot_lines` (`string[]`)

When `snapshot_format = "structured_entries_v1"`, MCP responses SHALL include:
- `snapshot_entries` (`object[]`)

For ACP look targets, MCP successful responses SHALL preserve relay-authored
additive freshness fields unchanged:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when relay omits)

`snapshot_format` determines payload variant; clients SHALL NOT infer variant
from transport heuristics.

#### Scenario: Preserve canonical tmux look payload unchanged

- **WHEN** `look` succeeds for tmux target
- **THEN** MCP returns `snapshot_format="lines"`
- **AND** includes canonical `snapshot_lines` payload
- **AND** ACP additive freshness fields are omitted

#### Scenario: Preserve structured-entries look payload unchanged

- **WHEN** `look` succeeds for an ACP-backed target
- **THEN** MCP returns `snapshot_format="structured_entries_v1"`
- **AND** preserves `snapshot_entries` ordering and values unchanged

### Requirement: MCP Authorization Adapter Boundary

MCP SHALL remain a request validator/adapter and SHALL perform no independent
authorization decisioning.
Relay SHALL remain the centralized authorization decision point.

#### Scenario: Propagate relay authorization denial unchanged

- **WHEN** relay returns `authorization_forbidden`
- **THEN** MCP returns the same code and details schema to caller
- **AND** MCP does not synthesize a custom authorization decision

### Requirement: MCP Control-to-Capability Mapping

MCP tool operations SHALL map to these canonical capability labels for
authorization outcomes:

- `list` -> `list.read`
- `send` -> `send.deliver`
- `look` -> `look.inspect`
- `do list` -> `do.list`
- `do show` -> `do.show`
- `do run` -> `do.run`
- `find` -> `find.query`

#### Scenario: Preserve look capability label in denial payload

- **WHEN** `look` is denied by relay policy
- **THEN** MCP denial details include `capability = "look.inspect"`

### Requirement: MCP ACP Look Success Passthrough

For ACP-backed look targets, MCP SHALL propagate relay-authored successful look
payloads unchanged.

MCP SHALL NOT synthesize ACP-specific adapter payloads for look results.
MCP SHALL NOT parse or reinterpret ACP `snapshot_entries` content.

#### Scenario: Preserve ACP snapshot entries without transformation

- **WHEN** caller invokes MCP `look` for ACP-backed target session
- **THEN** MCP returns successful look payload
- **AND** preserves ACP `snapshot_entries` ordering and values unchanged

#### Scenario: Preserve empty ACP structured snapshot payload unchanged

- **WHEN** relay returns successful ACP look payload with `snapshot_entries = []`
- **THEN** MCP propagates `snapshot_entries = []` unchanged

### Requirement: MCP List Sessions Selectors

`list` request parameters for principals listing SHALL be:

- `command` (required, must equal `"principals"`)
- `args` (optional object)
  - `namespace` (optional string)

`namespace` SHALL select the listing scope:

- omitted or null → associated/home bundle (default)
- a bundle name → that specific bundle
- `"GLOBAL"` → relay-wide principals
- `"*"` → adapter-owned fan-out across all namespaces

`"*"` SHALL be the only fan-out token; no `"ALL"` alias SHALL be accepted.
`"EXTERNAL"` and `"RELAY"` SHALL NOT be valid `list` selectors. The prior
`bundle_name` and `all` selectors are removed with no compatibility alias.

#### Scenario: Reject missing or unsupported list command

- **WHEN** caller omits `command` or provides a value other than `"principals"`
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Resolve home bundle when namespace omitted

- **WHEN** caller omits `namespace`
- **THEN** MCP resolves the associated/home bundle

#### Scenario: Select relay-wide principals with GLOBAL namespace

- **WHEN** caller provides `namespace="GLOBAL"`
- **THEN** MCP lists relay-wide principals

#### Scenario: Fan out across all namespaces with star token

- **WHEN** caller provides `namespace="*"`
- **THEN** MCP performs adapter-owned fan-out across all namespaces

#### Scenario: Reject ALL alias as fan-out token

- **WHEN** caller provides `namespace="ALL"`
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP List Sessions All-Mode Aggregation

All `list` `command="principals"` responses SHALL use the aggregate `bundles[]`
shape (see the Recipient Listing Contract); the `namespace` selector varies only
which bundles populate the array before the always-appended `GLOBAL` view.

When `namespace="*"`, MCP SHALL perform adapter-owned fanout in lexicographic
bundle-id order, populating `bundles[]` with every configured bundle followed by
the `GLOBAL` view.

Relay all-bundle list requests are not used; the relay accepts only a single
resolved namespace (a bundle name or `GLOBAL`) and never receives `"*"`.

On first `authorization_forbidden` during fanout, MCP SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first authorization denial in all-mode

- **WHEN** `namespace="*"` fanout encounters first `authorization_forbidden`
- **THEN** MCP stops fanout and returns non-aggregate error response

### Requirement: MCP List Sessions Unreachable Relay Fallback

MCP SHALL apply deterministic fallback behavior when a bundle relay is
unreachable.

When bundle relay is unreachable, MCP MAY synthesize canonical list payload only
for associated/home bundle using configuration + runtime reachability evidence.

If unreachable target is not associated/home bundle, MCP SHALL return
`relay_unavailable` and SHALL NOT synthesize cross-bundle payload.

In single-bundle mode, authorized home-bundle fallback SHALL return the canonical
aggregate `bundles[]` shape: the synthesized home bundle followed by the
always-appended `GLOBAL` view (itself synthesized as an empty `down` bundle when
the relay is unreachable).

In `namespace="*"` mode, encountering unreachable non-home bundle SHALL fail with
`relay_unavailable` and terminate fanout.

Home-bundle fallback startup-failure fields
(`startup_failure_count`, `recent_startup_failures`) SHALL be treated as
best-effort synthesized values from available local runtime state. When local
runtime failure history is unavailable, MCP SHALL return:

- `startup_failure_count=0`
- `recent_startup_failures=[]`

#### Scenario: Synthesize canonical home-bundle payload on unreachable relay

- **WHEN** caller requests associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns the home `bundles[]` entry with `state=down`
- **AND** includes required startup failure fields

#### Scenario: Default fallback startup-failure fields when local history is unavailable

- **WHEN** home-bundle fallback is synthesized
- **AND** local runtime startup-failure history cannot be read
- **THEN** MCP returns `startup_failure_count=0`
- **AND** returns `recent_startup_failures=[]`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns `relay_unavailable`

### Requirement: Advertise MCP raww tool

MCP tool inventory SHALL advertise top-level tool `raww` for direct single-
target raw writes.

#### Scenario: Include raww in tool inventory

- **WHEN** MCP client requests tool catalog
- **THEN** catalog includes `raww`

### Requirement: MCP raww request contract

MCP `raww` request fields SHALL be:
- `target_session` (required)
- `text` (required)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)

A bare `target_session` SHALL be qualified to the MCP server's bound bundle
before the relay call; a target that already carries an `@<namespace>` suffix is
forwarded verbatim. A bare target on a relay-wide (unassociated) MCP server SHALL
be rejected with `validation_unqualified_target`. Routing context for `raww` SHALL
then be inferred from the target's `@<namespace>` suffix. No explicit `namespace`
parameter is accepted.

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP raww sender authority

MCP raww sender identity SHALL be association-derived from MCP server context
and SHALL NOT be caller-overridable.

#### Scenario: Use association-derived sender for raww

- **WHEN** caller invokes MCP `raww`
- **THEN** MCP resolves sender principal from associated session context
- **AND** uses that principal for relay authorization/evaluation

### Requirement: MCP raww relay passthrough taxonomy

MCP raww SHALL preserve canonical relay codes and payload semantics for
validation and authorization failures, including:
- `validation_unknown_target`
- `validation_unsupported_namespace`
- `validation_invalid_params`
- `authorization_forbidden`

For denied raww requests, denial details SHALL preserve
`capability = "raww.write"`.

#### Scenario: Preserve raww denial capability label

- **WHEN** relay denies raww by policy
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

### Requirement: MCP raww success payload contract

MCP raww success responses SHALL preserve relay queued payload contract.

Required success fields:
- `status` (value `queued`)
- `target_session`
- `transport`

Optional fields:
- `request_id`
- `message_id`

#### Scenario: Return queued status for raww dispatch

- **WHEN** relay returns successful raww response
- **THEN** MCP returns `status = "queued"` with required fields intact

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

Successful response SHALL preserve the relay decision payload contract:

- `schema_version`
- `status`
- `choice_request_id`
- `outcome`
- optional `decided_by` (relay-derived, association-bound; present when the
  relay supplies it, omitted otherwise per the relay contract's
  skip-serializing semantics)
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

### Requirement: MCP Updown Tool

The system SHALL expose a meta-tool `updown` that administers the associated
bundle's runtime state. `updown` SHALL require `command`:

- `command="up"` requests hosting the associated bundle runtime.
- `command="down"` requests unhosting the associated bundle runtime.

`updown` SHALL address only the MCP server's associated bundle; cross-bundle
administration is out of scope. The deciding principal SHALL be the caller
session carried by the MCP server's Hello-established relay connection, and the
relay SHALL authorize it against the `updown` policy control (deny by default).

#### Scenario: Advertise updown tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `updown`

#### Scenario: Reject missing updown command selector

- **WHEN** a caller invokes `updown` without `command="up"` or `command="down"`
- **THEN** the tool returns `validation_invalid_params`

#### Scenario: Preserve updown authorization denial capability label

- **WHEN** the relay denies an `updown` request by policy
- **THEN** the tool returns `authorization_forbidden`
- **AND** denial details preserve `capability = "updown"`

### Requirement: MCP Updown Success Payload Contract

Successful `updown` responses SHALL preserve the relay bundle-transition payload
unchanged:

- `schema_version`
- `action`
- `bundles`
- `changed_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `changed_any`

#### Scenario: Return bundle-transition payload for updown

- **WHEN** an `updown` request succeeds
- **THEN** the response includes the required bundle-transition fields

### Requirement: MCP New Tool

The system SHALL expose a meta-tool `new` that registers a principal credential.
`new` SHALL require `command="peer"`.

`new peer` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `scope` (optional)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

The relay SHALL generate the PSK, persist only its SHA-256 hash, and route the
raw value to one of three credential destinations:

- **Response** (default, when neither option is set): the relay SHALL return the
  raw PSK once in the response.
- **Path** (`output_path` set): the relay SHALL write the PSK to that path and
  omit it from the response. The path MUST be absolute, its parent MUST already
  exist, and the target MUST NOT be a symlink; a path failing these
  preconditions SHALL be rejected with `validation_invalid_output_path`.
- **Config** (`write_to_config` set): the relay SHALL write the PSK to the
  principal's relay-owned canonical credential path and omit it from the
  response. Config is derivable only for **session** principals, whose
  credential location the relay owns; the relay SHALL reject Config for relay,
  user, or application principals with
  `validation_config_destination_unsupported`. Before deriving any path the
  relay SHALL reject a `principal_id` whose components are not a valid session
  identity — the configured session-id grammar for the id and the canonical
  bundle-name grammar for the namespace (which permits dotted names but rejects
  the traversal-only `.`/`..` segments and path separators) — with
  `validation_invalid_principal_id`.

The relay SHALL validate and stage the selected destination before mutating the
principal store; a rejected or failed destination SHALL NOT register the
principal. `new` is a relay-wide operation: the relay SHALL authorize the
connection principal against an `all`-scoped `new.peer` grant, and a
bundle-relative `home` grant SHALL be insufficient.

#### Scenario: Advertise new tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `new`

#### Scenario: Mint PSK for a new peer principal

- **WHEN** a caller invokes `new` with `command="peer"` and a `principal_id`
- **THEN** the relay registers the principal and returns the minted PSK
- **AND** omits the raw PSK from the response when `output_path` or
  `write_to_config` selected a file destination

#### Scenario: Reject config destination for a non-session principal

- **WHEN** a caller invokes `new` with `write_to_config` for a principal whose
  type is not session
- **THEN** the relay returns `validation_config_destination_unsupported`
- **AND** does not register the principal

#### Scenario: Reject mutually exclusive credential destinations

- **WHEN** a caller supplies both `output_path` and `write_to_config`
- **THEN** the request is rejected with `validation_invalid_params`
- **AND** no relay request is issued

### Requirement: MCP New Success Payload Contract

Successful `new` responses SHALL include:

- `schema_version`
- `principal_id`
- `principal_type`
- `config_snippet`
- `psk` (present only when the credential destination was Response)
- `written_path` (present only when the PSK was written to a file — the caller
  path for a Path destination, or the relay-owned canonical path for Config)

#### Scenario: Return minted credential payload for new peer

- **WHEN** a `new` `command="peer"` request succeeds
- **THEN** the response includes the required credential fields

### Requirement: MCP Change Tool

The system SHALL expose a meta-tool `change` that rotates an existing
principal's PSK. `change` SHALL require `command="psk"`.

`change psk` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

The relay SHALL generate a new PSK for the existing principal and apply the same
credential-destination selector as `new peer` (Response by default, Path for
`output_path`, Config for `write_to_config`), with identical session-only Config
derivation, path preconditions, and safe-segment validation. The relay SHALL
stage and commit the destination before revoking live connections that hold the
prior credential; a rejected or failed destination SHALL NOT rotate the PSK or
revoke any connection. `change` is a relay-wide operation: the relay SHALL
authorize the connection principal against an `all`-scoped `change.psk` grant,
and a bundle-relative `home` grant SHALL be insufficient.

#### Scenario: Advertise change tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `change`

#### Scenario: Rotate PSK for an existing principal

- **WHEN** a caller invokes `change` with `command="psk"` and a `principal_id`
- **THEN** the relay rotates the principal's PSK and returns the new value
- **AND** omits the raw PSK from the response when a file destination was
  selected

#### Scenario: Rejected destination leaves the credential unrotated

- **WHEN** a `change psk` request selects a destination the relay rejects
- **THEN** the relay returns the corresponding validation error
- **AND** does not rotate the PSK or revoke any live connection

### Requirement: MCP Change Success Payload Contract

Successful `change` responses SHALL include:

- `schema_version`
- `principal_id`
- `psk` (present only when the credential destination was Response)
- `written_path` (present only when the PSK was written to a file)

#### Scenario: Return rotated credential payload for change psk

- **WHEN** a `change` `command="psk"` request succeeds
- **THEN** the response includes the required rotated credential fields

