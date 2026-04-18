# mcp-tool-surface Specification

## Purpose
TBD - created by archiving change add-mcp-tool-surface-contract. Update Purpose after archive.
## Requirements
### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for relay MVP:

- `list`
- `send`

The relocked pre-stable MCP surface removes `list.sessions` with no
compatibility alias.

#### Scenario: Advertise relocked list meta-tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list`
- **AND** does not include `list.sessions`

### Requirement: Manual Bundle Configuration for MVP

The system SHALL treat bundle definitions as operator-managed configuration in
MVP and SHALL NOT expose MCP tools that mutate bundle configuration.

#### Scenario: Exclude configuration mutation tools from MCP surface

- **WHEN** an MCP client enumerates available tools
- **THEN** tool list excludes bundle mutation operations

### Requirement: Recipient Listing Contract

`list` with `command="sessions"` SHALL return bundle session listing payloads.

Single-bundle successful responses SHALL include:

- `schema_version`
- `bundle` object (`id`, `state`, `state_reason_code?`, `state_reason?`,
  `sessions[]`)

Each session entry SHALL include:

- `id`
- `name` (optional)
- `transport` (`tmux`|`acp`)

If requester identity is valid and policy denies relay-handled single-bundle
list access, MCP SHALL return `authorization_forbidden` and SHALL NOT return a
successful list payload.

#### Scenario: Deny single-bundle list request with authorization_forbidden

- **WHEN** requester identity is valid
- **AND** policy denies list visibility for requester
- **THEN** MCP returns `authorization_forbidden`
- **AND** does not return successful `bundle.sessions[]` output

### Requirement: Send Target Selection

`send` SHALL support exactly one target mode per request:

- `targets` (non-empty list of recipient identifiers)
- `broadcast=true` for full bundle delivery

`send` SHALL additionally support optional `delivery_mode` with values:

- `async`
- `sync`

If `delivery_mode` is omitted, the system SHALL default to `async`.

`send` timeout override fields SHALL be transport-specific:

- `quiescence_timeout_ms` (positive integer milliseconds) for tmux targets
- `acp_turn_timeout_ms` (positive integer milliseconds) for ACP targets

`send` SHALL reject conflicting timeout overrides in one request with
`validation_conflicting_timeout_fields`.

Transport-incompatible timeout overrides SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

`send` authorization scope SHALL follow requester policy control:

- `all:home`
- `all:all`

#### Scenario: Reject conflicting timeout override fields

- **WHEN** a caller provides `quiescence_timeout_ms` and
  `acp_turn_timeout_ms` in one request
- **THEN** the system rejects with `validation_conflicting_timeout_fields`

#### Scenario: Reject tmux timeout field on ACP send target

- **WHEN** request resolves target transport as ACP
- **AND** caller provides `quiescence_timeout_ms`
- **THEN** the system rejects with
  `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout field on tmux send target

- **WHEN** request resolves target transport as tmux
- **AND** caller provides `acp_turn_timeout_ms`
- **THEN** the system rejects with
  `validation_invalid_timeout_field_for_transport`

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
- `bundle_name`
- `request_id` (when provided by caller)
- `sender_session`
- `sender_display_name` (optional)
- `delivery_mode` (`async` or `sync`)
- `status`
- `results` (per-target entries)

In `delivery_mode=async`, `status` SHALL be `accepted` and each per-target
result SHALL include:

- `target_session`
- `message_id`
- `outcome` = `queued`

In `delivery_mode=sync`, `status` SHALL be one of `success`, `partial`, or
`failure`, and each per-target result SHALL include:

- `target_session`
- `message_id`
- `outcome` (`delivered`, `timeout`, or `failed`)
- `reason` (required when outcome is not `delivered`)

#### Scenario: Return accepted outcome for async request

- **WHEN** a caller invokes `send` with `delivery_mode=async`
- **THEN** the response status is `accepted`
- **AND** per-target outcomes are `queued`

#### Scenario: Return partial outcome for sync mixed delivery

- **WHEN** a caller invokes `send` with `delivery_mode=sync`
- **AND** at least one target succeeds and at least one target fails
- **THEN** `status` is `partial`
- **AND** each target result includes its own outcome and reason data

#### Scenario: Return empty results for zero effective recipients

- **WHEN** a caller invokes `send`
- **AND** effective target resolution yields zero recipients
- **THEN** the response includes `results=[]`
- **AND** `status` is `accepted` for `delivery_mode=async`
- **AND** `status` is `success` for `delivery_mode=sync`

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

#### Scenario: Unknown recipient error

- **WHEN** `send` targets a session that is not in the selected bundle
- **THEN** the tool returns error code `validation_unknown_recipient`
- **AND** includes a human-readable message

#### Scenario: Ambiguous recipient name error

- **WHEN** `send` targets a configured recipient name shared by multiple sessions
- **THEN** the tool returns error code `validation_ambiguous_recipient`
- **AND** includes matching session identifiers in error details

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

- `target_session` (required session identifier)
- `lines` (optional positive integer)
- `bundle_name` (optional; redundant under associated bundle context)

#### Scenario: Advertise look tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `look`

#### Scenario: Reject invalid lines in look request

- **WHEN** a caller provides `lines` outside valid range
- **THEN** the tool returns `validation_invalid_lines`

#### Scenario: Reject cross-bundle look request in MVP

- **WHEN** a caller provides `bundle_name` that differs from associated bundle
  context
- **THEN** the tool returns `validation_cross_bundle_unsupported`

#### Scenario: Reject unknown target

- **WHEN** caller requests inspection for unknown target session
- **THEN** tool returns `validation_unknown_target`

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

`snapshot_lines` ordering SHALL be oldest-to-newest.

#### Scenario: Return canonical inspection payload

- **WHEN** `look` succeeds
- **THEN** response includes canonical inspection payload fields
- **AND** `snapshot_lines` is returned as an array of strings ordered
  oldest-to-newest

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
payloads unchanged, including `snapshot_lines` ordering and emptiness semantics.

MCP SHALL NOT synthesize ACP-specific adapter payloads for look results.

#### Scenario: Return retained ACP snapshot lines from relay response

- **WHEN** caller invokes MCP `look` for ACP-backed target session with retained
  snapshot lines
- **THEN** MCP returns successful look payload
- **AND** `snapshot_lines` are relayed oldest -> newest without reordering

#### Scenario: Preserve empty ACP snapshot semantics

- **WHEN** relay returns successful ACP look payload with `snapshot_lines = []`
- **THEN** MCP propagates `snapshot_lines = []` unchanged

### Requirement: MCP ACP Sync Delivery-Phase Passthrough

For sync `send` targeting ACP transport, MCP SHALL propagate relay-authored
phase-1 acknowledgment details without adapter mutation.

When relay marks early delivery acknowledgment, MCP response SHALL preserve:

- `outcome = delivered`
- `details.delivery_phase = "accepted_in_progress"`
- unchanged `message_id` for request tracing

#### Scenario: Preserve early delivery-phase marker in MCP sync response

- **WHEN** relay returns sync ACP result with
  `details.delivery_phase = "accepted_in_progress"`
- **THEN** MCP returns the same result fields unchanged
- **AND** retains the same `message_id` in response payload

### Requirement: MCP List Sessions Selectors

`list` request parameters for MVP sessions listing SHALL be:

- `command` (required, must equal `"sessions"`)
- `args` (optional object)
  - `bundle_name` (optional)
  - `all` (optional bool; default `false`)

`bundle_name` and `all=true` SHALL be mutually exclusive.
If neither selector is provided, MCP SHALL resolve associated/home bundle.

#### Scenario: Reject missing or unsupported list command

- **WHEN** caller omits `command` or provides a value other than `"sessions"`
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Reject conflicting list selectors

- **WHEN** caller provides `bundle_name` and `all=true`
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP List Sessions All-Mode Aggregation

When `list` is called with `command="sessions"` and `all=true`, MCP SHALL perform
adapter-owned fanout in lexicographic bundle-id order and return aggregate
payload:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

Relay all-bundle list requests are not used in MVP.

On first `authorization_forbidden` during fanout, MCP SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first authorization denial in all-mode

- **WHEN** `all=true` fanout encounters first `authorization_forbidden`
- **THEN** MCP stops fanout and returns non-aggregate error response

### Requirement: MCP List Sessions Unreachable Relay Fallback

MCP SHALL apply deterministic fallback behavior when a bundle relay is
unreachable.

When bundle relay is unreachable, MCP MAY synthesize canonical list payload only
for associated/home bundle using configuration + runtime reachability evidence.

If unreachable target is not associated/home bundle, MCP SHALL return
`relay_unavailable` and SHALL NOT synthesize cross-bundle payload.

In single-bundle mode, authorized home-bundle fallback SHALL return canonical
single-bundle payload shape.

In `all=true` mode, encountering unreachable non-home bundle SHALL fail with
`relay_unavailable` and terminate fanout.

#### Scenario: Synthesize canonical home-bundle payload on unreachable relay

- **WHEN** caller requests associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns canonical single-bundle payload with `state=down`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns `relay_unavailable`

