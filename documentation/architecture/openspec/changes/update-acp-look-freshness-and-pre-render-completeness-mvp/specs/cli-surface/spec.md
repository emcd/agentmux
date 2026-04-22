## MODIFIED Requirements

### Requirement: CLI ACP Look Success Surface

For look success payloads, CLI machine output SHALL preserve relay payloads
unchanged, including discriminator and variant fields.

When relay returns tmux look payload:
- `snapshot_format="lines"` with `snapshot_lines`.

When relay returns ACP look payload:
- `snapshot_format="acp_entries_v1"` with `snapshot_entries`.

For ACP look responses, CLI machine output SHALL preserve relay additive
freshness fields unchanged:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when relay omits)

CLI MAY render ACP `snapshot_entries` with local presentation enhancements
(including ANSI/SGR styling), but wire/machine payloads SHALL remain unchanged.

#### Scenario: Preserve ACP structured payload in CLI machine output

- **WHEN** operator runs `agentmux look <target-session>` and ACP payload is
  returned from relay
- **THEN** CLI returns successful look payload unchanged
- **AND** includes `snapshot_format="acp_entries_v1"` and `snapshot_entries`

#### Scenario: Preserve stale-success with empty ACP snapshot entries

- **WHEN** operator runs `agentmux look <target-session>` for ACP target and
  relay returns stale-success with `snapshot_entries=[]`
- **THEN** CLI returns successful look payload
- **AND** includes required ACP freshness fields

#### Scenario: Preserve existing tmux look success path unchanged

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to tmux transport
- **THEN** CLI returns canonical successful look payload with
  `snapshot_format="lines"` and `snapshot_lines`
