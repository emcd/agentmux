## Context

Relay currently persists ACP look snapshot data to `state.json` to serve
`look` across relay restarts. This duplicates upstream ACP server state
and creates schema-migration burden every time the in-memory entry shape
evolves. Pre-MVP project posture explicitly discourages backwards-compat
infrastructure. The proposal originated from triage of repeated
`internal_unexpected_failure` events at bundle startup ("missing field
`call_id`" on stale `state.json`) and a coordinator/specialist
discussion that questioned the existence of the persistence layer
rather than patching the symptom.

Implementation owner: Backend Engineer. Surface confirmations from the
Agents Engineer (MCP) and Frontend Engineer (TUI). Coordinator notebook
record at `coordination/acp/3` (Rev3.1) carries the surface-lane
confirmations and the Backend Engineer second-look sign-off.

## Goals / Non-Goals

- Goals:
  - Single source of truth for ACP look snapshot (in-memory; ACP server
    is authoritative on reconnect).
  - Eliminate the schema-mismatch failure class on `state.json` parse.
  - Reconcile spec vocabulary with the coalesced `AcpSnapshotEntry`
    shape used by current code.
  - Pre-warm machinery already in place
    (`initialize_acp_target_for_startup`) becomes the sole rehydration
    path.
- Non-Goals:
  - Backwards compatibility for existing on-disk `state.json` files.
  - Look-path graceful-degradation hardening for future `state.json`
    parse failures of the much-shrunken file (tracked separately at
    `issues/relay/11` for relay-lane defensive-coding follow-up).
  - Changes to MCP/CLI/TUI look response semantics beyond the snapshot
    entry vocabulary reconciliation (coalesced `invocation` shape).

## Decisions

- Decision: drop persisted snapshot data; keep only `acp_session_id`.
  - Alternatives considered:
    - Keep persistence; add lenient deserialization for old formats.
      Rejected: bespoke compat infrastructure living forever after one
      use; pre-MVP discourages.
    - Keep persistence; harden look path to degrade on parse failure.
      Rejected: addresses the symptom, not the duplication of state
      that drives the failure class.
- Decision: fail+recreate migration posture; no compat layer.
  - Alternatives considered:
    - `#[serde(default)]` on dropped fields. Rejected: still adds
      compat surface; cost exceeds value at the upgrade moment.
- Decision: retain `call_id` on `AcpSnapshotEntry::Invocation` in the
  in-memory snapshot.
  - Rationale: `call_id` is the upstream-issued correlation token that
    pairs `tool_call` with its `tool_call_update`. The persistence-drop
    change does not require dropping the field from the in-memory entry
    — "stop persisting" is separable from "stop carrying entirely".
    Keeping `call_id` preserves diagnostic correlation in look output
    without re-introducing the schema-migration burden, since nothing
    in-memory crosses a serialization-version boundary.
  - Alternatives considered:
    - Drop the field as part of this change. Rejected: conflates two
      concerns (persistence layer vs in-memory data shape); loses
      provider correlation in the rendered transcript.
- Decision: introduce `worker_registry::set_state` API rather than
  open-coding state transitions across delivery code.
  - Alternatives considered:
    - Leave open-coded transitions in place after removing persistence.
      Rejected: ~10 call sites with swallowed errors becomes a
      maintenance hazard once persistence error-handling goes away.
- Decision: sequencing splits ACP lane (small accessor + cap discipline)
  from relay lane (wholesale rewiring).
  - Alternatives considered:
    - ACP lane owns the slim struct. Rejected:
      `PersistedAcpSessionState` lives in relay code; splitting causes
      a half-state where struct is slim but `acp_delivery.rs` still
      writes dropped fields.

## Risks / Trade-offs

- Risk: First `look` after relay restart returns fresh-but-empty for
  the duration of upstream `session/load`.
  - Mitigation: existing freshness vocabulary
    (`acp_worker_initializing`, `acp_snapshot_prime_timeout`) already
    covers this; operators see a clear stale-reason rather than an
    error.
- Risk: `state.json` parse failure at upgrade moment causes loss of
  upstream conversation continuity (one extra `session/new` per ACP
  target).
  - Mitigation: acceptable pre-MVP; release-note guidance ("delete
    `state.json` for a clean start") covers operator path.
- Trade-off: in-memory cap-and-evict at 1000 mirrors current
  persisted-path retention. If retention proves too low for typical
  conversations, raise the cap; not a structural change.

## Migration Plan

Backend Engineer owns implementation across both modules. The work is
split into two sequential PRs to avoid a half-state where the persisted
struct is slim but `acp_delivery.rs` still writes dropped fields.

1. First PR (ACP module changes; small, isolated): non-draining
   accessor on `AcpStdioClient` + buffer cap-and-evict discipline.
2. Second PR (relay module changes; wholesale): slim struct + rewire
   all disk-read sites + remove persistence calls + new registry API
   + tests.
3. After deployment, on first relay restart per environment, any
   pre-existing `state.json` that fails new-shape parse is logged and
   deleted; worker creates a new upstream ACP session via `session/new`.
   No operator action required beyond awareness.

Rollback: revert both PRs in reverse order. State files written under
the new shape (`schema_version = 2`) will fail to parse under the
reverted relay code; same fail+recreate pattern applies.
