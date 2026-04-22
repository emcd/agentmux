# Change: Relock ACP look freshness and structured ACP conversation payloads

## Why

ACP look snapshots are still confusing in practice: freshness signaling can
oscillate around busy/idle transitions, and line-level payloads do not
consistently preserve typed conversation structure (user/assistant/thinking/tool
activity) the way `agentmux-acp` does.

## What Changes

- Relock ACP look freshness with explicit deterministic predicate ordering and
  source timestamp precedence.
- Relock ACP look response for ACP targets to structured snapshot entries with
  explicit kind metadata and discriminator field.
- Keep tmux look payload line-based and unchanged in shape.
- Lock one authoritative ACP replay ingestion/write path for both
  `session/load` baseline replay and live `session/update` ingestion.
- Relock MCP and CLI ACP look adapter behavior to preserve structured payloads
  unchanged and keep additive freshness fields.
- Lock compatibility handoff from legacy flattened ACP snapshots to canonical
  structured ACP snapshot entries.

## Breaking Changes (pre-MVP intentional)

- ACP look success payload moves from line-only shape to discriminated
  structured payload:
  - old: `snapshot_lines` for ACP targets,
  - new: `snapshot_format = "acp_entries_v1"` with `snapshot_entries`.
- tmux look remains `snapshot_format = "lines"` with `snapshot_lines`.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code (implementation follow-up):
  - `src/acp/` shared replay-to-structured conversion
  - `src/relay/delivery/acp_delivery.rs` ingestion/write path
  - `src/relay/delivery/acp_state.rs` freshness derivation and
    replace-on-first-structured-load handoff behavior
  - MCP/CLI look passthrough and regression coverage
