## MODIFIED Requirements

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `acp_entries_v1`)

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

#### Scenario: Preserve ACP structured payload unchanged

- **WHEN** relay returns successful ACP look payload with
  `snapshot_format="acp_entries_v1"`
- **THEN** MCP returns the same `snapshot_format` and `snapshot_entries`
  unchanged

#### Scenario: Preserve required ACP freshness fields for empty snapshot entries

- **WHEN** relay returns ACP look payload with `snapshot_entries=[]`
- **THEN** MCP response still includes required `freshness` and
  `snapshot_source`

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
