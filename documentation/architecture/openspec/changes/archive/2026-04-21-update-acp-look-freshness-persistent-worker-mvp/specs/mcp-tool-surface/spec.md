## MODIFIED Requirements

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

For ACP look targets, MCP successful responses SHALL preserve relay-authored
additive freshness fields unchanged:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional)

`snapshot_lines` ordering SHALL be oldest-to-newest.

#### Scenario: Preserve canonical tmux look payload unchanged

- **WHEN** `look` succeeds for tmux target
- **THEN** MCP returns canonical look payload fields
- **AND** ACP additive freshness fields are omitted

#### Scenario: Preserve ACP additive freshness fields unchanged

- **WHEN** relay returns successful ACP look payload with freshness fields
- **THEN** MCP returns the same freshness fields unchanged
- **AND** does not translate stale-success payload into an error

#### Scenario: Preserve required ACP freshness fields for empty snapshot

- **WHEN** relay returns ACP look payload with `snapshot_lines=[]`
- **THEN** MCP response still includes required `freshness` and
  `snapshot_source`
