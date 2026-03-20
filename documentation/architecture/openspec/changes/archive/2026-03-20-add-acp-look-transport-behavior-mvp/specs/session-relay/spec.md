## ADDED Requirements

### Requirement: ACP Look Snapshot Contract

Relay look SHALL support ACP-backed target sessions using relay-managed
snapshot state populated from ACP prompt-turn updates.

For ACP targets, relay SHALL:
- ingest non-empty text lines from ACP `session/update` payloads during
  prompt turns
- persist those lines in per-session runtime state
- retain at most 1000 lines per session
- evict oldest lines first when retention exceeds 1000
- return look results ordered oldest -> newest
- return tail lines based on requested `lines`
- return success with `snapshot_lines = []` when no retained snapshot exists

#### Scenario: Return ACP look snapshot from retained updates

- **WHEN** requester invokes relay `look` for a target session backed by ACP
  transport after ACP prompt turns emitted `session/update` text
- **THEN** relay returns successful look payload with retained `snapshot_lines`
- **AND** `snapshot_lines` are ordered oldest -> newest

#### Scenario: Enforce bounded ACP look retention and oldest-first eviction

- **WHEN** retained ACP snapshot lines for one target exceed 1000
- **THEN** relay evicts oldest lines first
- **AND** subsequent look requests return at most 1000 retained lines

#### Scenario: Return empty ACP look snapshot when no update lines exist

- **WHEN** requester invokes relay `look` for ACP target with no retained
  snapshot state
- **THEN** relay returns successful look payload with `snapshot_lines = []`

#### Scenario: Preserve existing tmux look behavior unchanged

- **WHEN** requester invokes relay `look` for a target session backed by tmux
  transport
- **THEN** relay executes canonical look capture behavior unchanged
