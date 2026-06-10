## MODIFIED Requirements

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

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `acp_entries_v1`)

`bundle_name` is retired from look responses; bundle context is recoverable
from the `target_session` suffix.

When `snapshot_format = "lines"`, MCP responses SHALL include:
- `snapshot_lines` (`string[]`)

When `snapshot_format = "acp_entries_v1"`, MCP responses SHALL include:
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
- `pending_requests[]` ordered by enqueue `sequence` ascending

`bundle_name` is retired from grant list responses; bundle context is implicit
from the associated bundle connection.

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
