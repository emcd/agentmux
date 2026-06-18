# Change: Unify the look path across transports and relocate look vocabulary

## Why

The relay look handler (`src/relay/handlers/look.rs`) still branches on transport
identity (`TargetConfiguration::Tmux` vs `Acp`) to shape its snapshot — exactly
the pattern the `decouple-transport-layer` refactor was meant to eliminate.
`Transport::give_output` was designed for polymorphic look dispatch, but the
tmux side was never wired up: `TmuxTransport::give_output` returns `None` and the
handler falls back to an inline pane capture.

At the same time, the look-snapshot vocabulary is not transport-neutral. The
structured entry type (`AcpSnapshotEntry`) and `ToolCallStatus` live in
`src/acp/`, so the transport-level `LookSnapshotPayload` reaches up into a
concrete transport for its leaf type. That keeps the look vocabulary out of the
acp-free `src/transports/vocabulary.rs` layer where the freshness/source enums
already live.

## What Changes

- **Relocate look vocabulary into the acp-free layer.** Move the structured
  entry type (`AcpSnapshotEntry` → `StructuredEntry`), `ToolCallStatus`, and the
  transport-level `LookSnapshotPayload` enum into `src/transports/vocabulary.rs`.
  `src/acp` then *produces* `transports::StructuredEntry` from its `ReplayEntry`
  intermediate (which stays in `src/acp`). Rename `AcpLookFreshness` →
  `LookFreshness`, `AcpLookSnapshotSource` → `LookSnapshotSource` (subsumes
  `ideas/transport/5`).
- **Unify look dispatch at the `OutputView` seam.** Add a single
  `get_output_view(member, runtime_directory)` accessor that returns an
  `Arc<dyn OutputView>` for any lookable transport, then collapse the look
  handler to one polymorphic `get_output_view(...)?.look(mode)` call with no
  transport-identity arm for snapshot shaping.
- **Implement `TmuxOutputView`.** A config-constructed view holding the tmux
  socket path + session id; `look()` performs the existing
  `resolve_active_pane_target` + `capture_pane_tail_lines` capture and returns
  `LookSnapshotPayload::Lines`. The accessor is structured to accept **either**
  provenance — a worker-published registry handle (ACP today, future streaming
  tmux) or a config-constructed view (tmux today) — so a future stateful tmux
  worker can publish a handle without reworking the seam.
- **Per-transport `LookMode` validation.** The `offset`-unsupported check for
  tmux moves into `TmuxOutputView::look` as a `TransportError`, and the handler
  maps validation-class transport error codes to relay validation errors instead
  of blanket `internal_unexpected_failure`.
- **BREAKING (wire): rename the look snapshot discriminator.**
  `snapshot_format` value `acp_entries_v1` → `structured_entries_v1` in the relay
  and MCP look response contracts. The transport-level enum variant is
  `StructuredEntries`. The structured entry `kind` tags
  (`user`/`agent`/`cognition`/`invocation`/`update`) are unchanged.

## Impact

- Affected specs:
  - `transport-abstraction` — MODIFIED look-handle requirement; ADDED
    transport-neutral look vocabulary requirement.
  - `session-relay` — MODIFIED look response contract (discriminator rename).
  - `mcp-tool-surface` — MODIFIED MCP look response contract (discriminator
    rename).
  - `cli-surface` — MODIFIED CLI ACP look success surface (discriminator rename).
- Affected code:
  - `src/transports/vocabulary.rs`, `src/transports/contract.rs` — new home for
    `StructuredEntry`, `ToolCallStatus`, transport-level `LookSnapshotPayload`,
    renamed freshness/source enums.
  - `src/acp/` (`render.rs`, `mod.rs`, `state.rs`, `transport.rs`, `client.rs`) —
    produce `transports::StructuredEntry`; `ReplayEntry` stays local.
  - `src/tmux/transport.rs` — `TmuxOutputView`.
  - `src/relay/delivery/` — `get_output_view` accessor over the worker registry +
    config construction.
  - `src/relay/handlers/look.rs` — collapse to one polymorphic call.
  - `src/relay/contract.rs` — wire discriminator rename.
  - `src/commands/look.rs` — CLI look handler binds the variant + emits the
    literal (compile-coupled with the relay rename).
  - `tests/integration/mcp/look.rs`, `tests/integration/cli/look.rs`,
    `tests/integration/acp/look.rs`, `tests/unit/relay.rs` — construct the
    variant and/or assert the `acp_entries_v1` literal.
  - `src/mcp/README.md`, `src/tui/README.md`, `documentation/usage/tui.md` —
    same-commit wire-key wording sync.
- Cross-lane consumers of the wire break (coordinated, not decided here): the
  TUI (`src/tui/state/`, `src/tui/render/interaction.rs`) names the entry type
  and discriminator directly for rich rendering; the MCP look handler
  (`src/mcp/server/handlers/look.rs`) and the CLI look handler
  (`src/commands/look.rs`) both build their response off `snapshot_format`. The
  rename is compile-coupled across the single crate, so the drop must land
  atomically. Per the raww/82 precedent (CLI + MCP vertical assigned to one
  owner), the lane split for the CLI handler, its integration tests, and the
  `mcp-tool-surface`/`cli-surface` spec/README sync is a **Coordinator dispatch
  decision**, not fixed by this proposal.

## Out of Scope

- `ideas/transport/6` (capability-gate collapse) — left as a separate change.
- A stateful streaming-tmux worker — only the accessor's dual-provenance shape is
  prepared here; no tmux worker is added.
