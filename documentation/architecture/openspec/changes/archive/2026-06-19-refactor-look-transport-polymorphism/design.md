## Context

The look path is the last place transport-identity branching survives the
`decouple-transport-layer` refactor. The handler shapes its snapshot with an
explicit `match &target.target { Tmux => capture_pane..., Acp => view.look() }`,
and `TmuxTransport::give_output` returns `None`. Separately, the look-snapshot
leaf type (`AcpSnapshotEntry`) and `ToolCallStatus` live in `src/acp/`, so the
transport-level `LookSnapshotPayload` (and the relay wire payload) depend on a
concrete transport for their structured variant.

Note on layering: `src/transports/contract.rs` already imports
`crate::acp::{AcpWorkerDriver, AcpDriverServices}` and `crate::tmux::TmuxTransport`
because `TransportImpl` is the static-dispatch enum over the concrete transports.
So `transports → acp` is a sanctioned edge for dispatch; it is **not** the
forbidden edge. The forbidden edge is `transports/acp/tmux → relay`, which stays
at zero. The architectural goal here is narrower and sharper: keep the look
vocabulary in the acp-free `src/transports/vocabulary.rs` sub-layer (which imports
only `serde` today), so the look snapshot types carry no concrete-transport
coupling at all.

## Goals / Non-Goals

- Goals:
  - One polymorphic look call in the relay handler; no transport-identity arm for
    snapshot shaping.
  - Transport-neutral look vocabulary living in the acp-free vocabulary layer.
  - A `get_output_view` accessor whose shape already accommodates both handle
    provenances (worker-published, config-constructed).
- Non-Goals:
  - Adding a stateful tmux worker / streaming tmux output (future).
  - Capability-gate collapse (`ideas/transport/6`).
  - Changing the structured entry `kind` set or ACP worker buffer mechanics.

## Decisions

- **Decision: relocate `StructuredEntry` + `ToolCallStatus` + transport-level
  `LookSnapshotPayload` into `src/transports/vocabulary.rs`.** The entry kinds
  (`user`/`agent`/`cognition`/`invocation`/`update`) describe a structured agent
  transcript, not ACP wire framing; `invocation`'s `call_id`/`status`/`result`
  are general tool-use semantics. `ReplayEntry` (the ACP-specific intermediate)
  stays in `src/acp`; `replay_entries_to_snapshot_entries` stays in `src/acp` as
  the acp→neutral mapping, now producing `transports::StructuredEntry`.
  `ToolCallStatus` is consumed only inside `src/acp` today, so the move is clean.
  - Alternatives considered: leave the leaf in `src/acp` and accept the
    `contract.rs → acp` import. Rejected — it keeps `LookSnapshotPayload` out of
    the acp-free layer and conflates "dispatch needs to name transports" with
    "vocabulary leaks a transport type."

- **Decision: unify at `OutputView`, not at `give_output`/registry, for tmux.**
  `get_output_view(member, runtime_directory)` resolves a handle by provenance:
  (1) a worker-published handle from the delivery registry if present (ACP today,
  future streaming tmux), else (2) for a tmux member, a config-constructed
  `TmuxOutputView { socket_path, session_id }`. The handler then calls
  `.look(mode)` once.
  - Why not publish a tmux handle through the worker (the originally-proposed
    step)? Tmux look has no worker-owned state — it is a stateless capture
    against the tmux socket — and tmux sessions are lookable **independent of
    worker lifecycle**: the capture reads the socket directly, not any
    worker-held buffer, so it works whether or not a worker has spawned for the
    session. Registry-publishing would couple look availability to startup
    ordering for only cosmetic symmetry. The honest asymmetry (ACP handle is
    stateful/worker-owned; tmux handle is stateless/config-derived) is reflected,
    and the registry-first accessor already absorbs a future tmux worker that
    *does* publish.
  - Note on tmux worker spawn timing: whether generic tmux workers spawn lazily
    (today) or move to ACP-style startup-spawn-with-readiness-gate is a separate
    lifecycle question (adjacent to `ideas/transport/6`), **out of scope here**.
    It does not affect this accessor: because tmux look is socket-direct rather
    than worker-state-derived, the config-constructed `TmuxOutputView` is correct
    under either spawn policy, and a future stateful/streaming tmux worker drops
    in additively via the registry-first branch.
  - `TmuxTransport::give_output` therefore continues to return `None`: tmux output
    is not worker-owned today.

- **Decision: per-transport `LookMode` validation.** `TmuxOutputView::look`
  returns `TransportError { code: "validation_offset_unsupported" }` when
  `offset > 0` (tmux has no offset semantics). The handler maps validation-class
  transport error codes to relay validation errors; non-validation codes keep
  mapping to `internal_unexpected_failure`.

- **Decision: keep two `LookSnapshotPayload` types.** The transport-level enum
  (in `vocabulary.rs`, serde-carrying for its leaf but format-agnostic) and the
  relay wire enum (in `src/relay/contract.rs`, owning the `snapshot_format`
  discriminator) stay distinct; only the leaf entry type is shared. This
  preserves the layering where the relay owns the wire format. The existing
  `transport_acp_snapshot_to_wire` translation stays, renamed and trivial.

- **Decision: `snapshot_format` value `acp_entries_v1` → `structured_entries_v1`.**
  Keep the `_v1` version suffix as cheap insurance against a second wire break
  before 1.0 (per Coordinator). Transport-level variant is `StructuredEntries`
  (no suffix; in-memory).

- **Decision: entry leaf type named `StructuredEntry`.** "Snapshot" is accurate
  only for today's render-the-replay-buffer model; once ACP performs real
  tool-call-result and assistant-response aggregation, the entries are cumulative
  transport state, not a snapshot of a stream — so `Snapshot` would mis-describe
  the type. `StructuredEntry` also completes the naming symmetry across the three
  layers: leaf `StructuredEntry`, transport variant `StructuredEntries`, wire
  `structured_entries_v1`.

## Risks / Trade-offs

- **Breaking wire change across lanes.** The discriminator rename and the
  entry-type rename/relocation touch the TUI (rich rendering) and the MCP look
  handler. → The relay change and the FE/MCP consumer updates must land together;
  sequence the merge so no consumer deserializes the old discriminator against a
  new relay (or vice versa). Pre-1.x alpha, live releases — no compatibility
  shim; clients move with the relay.
- **Accessor provenance precedence.** Registry-first means a stale published
  handle would shadow a config-constructed one. → Only ACP publishes today, and
  ACP has no config-constructed fallback, so there is no precedence ambiguity for
  current transports; the ordering is defined now so a future tmux worker is
  additive.

## Migration Plan

1. Land vocabulary relocation + renames (compile-coupled within the single
   crate; no behavior change).
2. Land `TmuxOutputView` + `get_output_view` accessor + handler collapse.
3. Land the wire discriminator rename together with the TUI + MCP consumer
   updates (lane-coordinated single landing).
4. Update the live `session-relay`, `transport-abstraction`, and
   `mcp-tool-surface` specs in the same change as implementation.

No rollback hedge beyond normal revert: the change is one bundled
contract+consumer move on alpha.

## Open Questions

- Whether to also re-title the `session-relay` "ACP Look Snapshot Contract"
  requirement (its prose says "canonical ACP snapshot entry vocabulary"). Left
  unchanged here: it describes ACP worker-buffer mechanics that are genuinely
  ACP-specific, and the entry kinds are identical — only the Rust type's home and
  name change. The transport-neutral vocabulary is captured in the new
  `transport-abstraction` requirement instead.
