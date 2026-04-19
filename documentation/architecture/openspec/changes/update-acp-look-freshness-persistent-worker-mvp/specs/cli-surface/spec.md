## MODIFIED Requirements

### Requirement: CLI ACP Look Success Surface

For ACP-backed look targets, CLI `agentmux look` SHALL surface relay-authored
successful look payloads and return zero exit status.

CLI SHALL preserve relay `snapshot_lines` ordering and emptiness semantics.

For ACP look responses, CLI machine output SHALL preserve relay additive
freshness fields unchanged:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional)

CLI MAY render freshness summaries in human-oriented output as additive
presentation only.

#### Scenario: Surface retained ACP snapshot lines with fresh metadata

- **WHEN** operator runs `agentmux look <target-session>` and ACP payload is
  fresh
- **THEN** CLI returns successful look payload
- **AND** includes required ACP freshness fields

#### Scenario: Surface empty ACP snapshot as successful stale payload

- **WHEN** operator runs `agentmux look <target-session>` for ACP target and
  relay returns stale-success with `snapshot_lines=[]`
- **THEN** CLI returns successful look payload
- **AND** includes required ACP freshness fields

#### Scenario: Preserve existing tmux look success path unchanged

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to tmux transport
- **THEN** CLI returns canonical successful look payload
