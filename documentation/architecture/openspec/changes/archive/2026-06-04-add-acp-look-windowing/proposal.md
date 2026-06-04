# Change: Add ACP look entry windowing with offset and truncation metadata

## Why

The `look` relay/MCP tool returns the full ACP replay buffer as a single
payload. For active sessions (100+ entries) this routinely exceeds the ~25K MCP
response limit, so consumers see only the oldest entries — the opposite of
useful. The tmux look path already truncates via `lines`; the ACP path had no
equivalent windowing nor any signal that the view was truncated.

## What Changes

- Add optional `offset` to the relay `look` request. For ACP targets it pages
  the entry window backward from the newest end of the replay buffer; for tmux
  targets only `0` is valid and a nonzero offset is rejected with
  `validation_offset_unsupported`.
- Apply transport-keyed default window sizes: tmux `lines = 120` (unchanged),
  ACP `lines = 50` (new) — ACP replay entries are far larger than tmux lines,
  so the smaller default keeps responses under the MCP payload limit.
- Window ACP entries deterministically with two saturating subtractions:
  `end = entries_total.saturating_sub(offset)`,
  `start = end.saturating_sub(lines)`. An offset beyond the buffer yields an
  empty window (a normal terminal page), not an error.
- Add required `entries_total` and `returned_entries_count` to ACP look responses so
  callers can detect truncation and bound backward walks; `returned_entries_count`
  equals the length of `snapshot_entries`, and `entries_total` reflects the full
  buffer on every response including stale and empty snapshots.
- Surface `offset` through the MCP `look` tool and `entries_total` /
  `returned_entries_count` through the MCP and CLI look responses.

## Impact

- Affected specs: session-relay (Relay Look Operation, Look Capture Window
  Bounds, Look Response Contract).
- Affected code: `src/relay/{contract,context,handlers}.rs`,
  `src/relay/delivery/acp_state.rs`, `src/mcp/{params,server}.rs`,
  `src/commands/look.rs`, `src/tui/state/compose.rs`, `src/mcp/README.md`;
  tests under `tests/integration/{acp,mcp,cli}/look.rs`,
  `tests/unit/relay.rs`, `tests/integration/session_relay_look.rs`.
- Resolves: `issues/mcp/2`.
