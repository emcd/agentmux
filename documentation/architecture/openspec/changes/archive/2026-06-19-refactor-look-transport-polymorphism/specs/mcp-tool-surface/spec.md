## MODIFIED Requirements

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
