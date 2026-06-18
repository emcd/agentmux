## 1. Relocate look vocabulary into the acp-free layer

- [x] 1.1 Move `ToolCallStatus` from `src/acp/mod.rs` into `src/transports/vocabulary.rs`; update `src/acp` consumers (`render.rs`, `client.rs`, `mod.rs`) to import `transports::ToolCallStatus`.
- [x] 1.2 Move the structured entry type into `src/transports/vocabulary.rs`, renaming `AcpSnapshotEntry` → `StructuredEntry` (kinds unchanged: `user`/`agent`/`cognition`/`invocation`/`update`).
- [x] 1.3 Rename `AcpLookFreshness` → `LookFreshness` and `AcpLookSnapshotSource` → `LookSnapshotSource` in `src/transports/vocabulary.rs`; update all references.
- [x] 1.4 Move the transport-level `LookSnapshotPayload` enum into `src/transports/vocabulary.rs`, renaming the `AcpEntries` variant → `StructuredEntries`; drop the `crate::acp::AcpSnapshotEntry` import from `src/transports/contract.rs`.
- [x] 1.5 Keep `ReplayEntry` in `src/acp`; update `replay_entries_to_snapshot_entries` (and `snapshot_entries_to_plain_lines`) in `src/acp/render.rs` to produce/consume `transports::StructuredEntry`.

## 2. Implement TmuxOutputView and the unified accessor

- [x] 2.1 Implement `TmuxOutputView { socket_path, session_id }` in `src/tmux/transport.rs` with `OutputView::look` performing `resolve_active_pane_target` + `capture_pane_tail_lines`, returning `LookSnapshotPayload::Lines`.
- [x] 2.2 In `TmuxOutputView::look`, return `TransportError { code: "validation_offset_unsupported" }` when `LookMode::offset > 0`.
- [x] 2.3 Add `get_output_view(member, runtime_directory)` in `src/relay/delivery/` returning `Option<Arc<dyn OutputView>>`: registry-published handle first (ACP today, future tmux), else a config-constructed `TmuxOutputView` for tmux members; `None` for non-lookable types.
- [x] 2.4 Leave `TmuxTransport::give_output` returning `None` (tmux output is not worker-owned today); confirm no worker publishes a tmux handle.

## 3. Collapse the relay look handler

- [x] 3.1 Replace the transport-identity `match` in `execute_look` (`src/relay/handlers/look.rs`) with a single `get_output_view(...)?.look(mode)` call.
- [x] 3.2 Map validation-class `TransportError` codes (e.g. `validation_offset_unsupported`) to relay validation errors; keep non-validation codes mapping to `internal_unexpected_failure`.
- [x] 3.3 Preserve the missing-handle behavior (unstarted/failed/respawning ACP worker → empty stale/unavailable snapshot) via the accessor returning the published ACP handle or a stale fallback.
- [x] 3.4 Keep the per-transport default window sizes (`LOOK_LINES_DEFAULT` for lines, `ACP_LOOK_ENTRIES_DEFAULT` for entries) and the shared `LOOK_LINES_MAX` bound.

## 4. Wire discriminator rename

- [x] 4.1 Rename the relay wire `LookSnapshotPayload::AcpEntriesV1` variant / `snapshot_format` value to `structured_entries_v1` in `src/relay/contract.rs`.
- [x] 4.2 Rename and simplify `transport_acp_snapshot_to_wire` (`src/relay/handlers/look.rs`) to map `StructuredEntries` → `structured_entries_v1`.
- [x] 4.3 Update the `relay.look.response` inscription `snapshot_format` label.

## 5. Cross-lane consumer updates (coordinated landing)

- [x] 5.1 Update the MCP look handler (`src/mcp/server/handlers/look.rs`) to the `structured_entries_v1` discriminator.
- [x] 5.2 Update the TUI look consumers (`src/tui/state/mod.rs`, `src/tui/state/compose/interaction.rs`, `src/tui/render/interaction.rs`) to import `transports::StructuredEntry` and decode `structured_entries_v1`.
- [x] 5.3 Update the CLI look handler (`src/commands/look.rs`) — it binds the `AcpEntriesV1` variant and emits the `acp_entries_v1` literal (compile-coupled with the relay rename).
- [x] 5.4 Sync wire-key wording in docs touched by the rename: `src/mcp/README.md`, `src/tui/README.md`, `documentation/usage/tui.md` (`acp_entries_v1` → `structured_entries_v1`; "ACP entries" → structured entries).

## 6. Tests and spec sync

- [x] 6.1 Update/extend look tests: tmux `Lines`, ACP `StructuredEntries`, tmux `offset > 0` → validation error, missing ACP handle → stale snapshot.
- [x] 6.2 Update wire/serde fixtures and assertions referencing `acp_entries_v1`/`AcpEntriesV1` to `structured_entries_v1`/`StructuredEntries`: `tests/integration/mcp/look.rs`, `tests/integration/cli/look.rs`, `tests/integration/acp/look.rs`, `tests/unit/relay.rs`.
- [x] 6.3 Confirm `src/transports/vocabulary.rs` imports no concrete transport; confirm no `transports/acp/tmux → relay` edge introduced.
- [x] 6.4 Update the live `session-relay`, `transport-abstraction`, `mcp-tool-surface`, and `cli-surface` specs to match (this change's deltas).
- [x] 6.5 `cargo fmt`, `cargo clippy --all-targets` (`-D warnings`), full `cargo test` green.
