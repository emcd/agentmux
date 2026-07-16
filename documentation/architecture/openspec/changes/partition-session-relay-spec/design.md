# Design: Partition session-relay specification

## Context

`openspec/specs/session-relay/spec.md` was the single OpenSpec capability
spec for the relay operation surface -- the largest live capability (94
requirements) and the destination for the majority of archived changes.
Reviewers comparing or extending related clauses had to scan all 3771
lines; the 3000-line separation between the async-only submit requirement
(L338-344) and the synchronous-propagation clause (L3347-3351) let a
self-contradiction slip through review. Length hides defects.

## Goal

Reorganize the 94 requirements into capability-scoped sibling specs without
altering their normative content. Each reviewer of a future change should
be able to read the relevant partition spec (typically 7-27 requirements)
instead of scanning the full file.

## Non-Goals

- No semantic changes to any requirement.
- No new requirements added (one exception: the new hub requirement
  documenting the partition index is a meta-requirement about the spec
  itself, not the relay domain).
- No deletion of any requirement (per `## REMOVED Requirements` semantics;
  all 94 are relocated, none retired).
- No change to the OpenSpec change schema or workflow.
- No compression or deduplication of requirement text in this pass. The
  dispatch says compress, but in-scope compression would change semantics;
  surface any candidates as separate OpenSpec changes.

## Decisions

### D1 -- Partition by capability, not by transport or by request type

The dispatch's illustrative cut (addressing/routing, delivery & quiescence,
transport contracts, authorization/scope, envelope format) is the starting
point. The final partition is the editor lane's call: 8 partitions chosen
to keep cross-cutting concepts in their dominant capability and avoid
recreating the discoverability problem inside any single partition.

### D2 -- Cross-cutting requirements stay with their dominant capability

Specific moves per BE review feedback (post-initial-proposal):

- `Relay raww target resolution and bundle boundary` -> `addressing-routing`
  (target resolution is routing).
- `Relay raww authorization mapping` -> `authorization-scope`
  (authorization mapping is auth).
- `ACP Look Snapshot Contract` -> `look-and-stream-events`
  (Look-specific snapshot).
- `ACP Look Freshness Derivation` -> `look-and-stream-events`
  (Look-specific freshness).
- `Relay About Operation`, `Relay About Response Contract` ->
  `transport-contracts` (operation contracts).
- `Pty Prime Timeout`, `Pty Wedged State Detection`,
  `Pty Default Per-Coder Dimensions` -> `transport-contracts`
  (Pty-specific execution contracts).
- 10 `embeddable-runtime-api` ADDED requirements -> delta spec path
  migrated to a new `runtime-api` directory under that change. The
  partition itself is created on `embeddable-runtime-api`'s archive
  (opsx-sync creates the live spec on ADD); this partition change does
  NOT pre-create the live spec (BE BLOCKER 1 review feedback).

### D3 -- Hub at `openspec/specs/session-relay/spec.md`

OpenSpec requires >=1 `### Requirement:` per spec. The hub file contains
exactly one new requirement (`Session-Relay Specification Partition Index`)
plus prose: a partition index table, the relay/53 archive-order
relocations (covering ADDED + MODIFIED), and the active-change migration
obligation. The normative migration rule explicitly covers both `## MODIFIED
Requirements` and `## ADDED Requirements` deltas.

### D4 -- Verbatim preservation

Every requirement block (heading + body + scenarios) moves byte-for-byte.
Verification: `.auxiliary/scribbles/verify_split_v3.py` confirms sha256 of
every block matches the pre-split text. No whitespace, line-ending, or
punctuation drift.

### D5 -- Atomic active-change migration

Seven active OpenSpec changes (`add-container-sandboxing`,
`add-do-action-tool`, `add-pty-transport`,
`add-about-surface-and-description-fields`, `deliver-async-terminal-outcomes`
[relay/53], `embeddable-runtime-api`, `add-e2e-test-harness`) have delta
specs at `<change>/specs/session-relay/spec.md`. Without atomic migration,
those delta spec paths are stale as soon as this change lands on master,
and a later archive by any of those changes would attempt to apply the
delta to the now-empty session-relay hub -- breaking or, worse, silently
repopulating the hub with the moved requirements.

This change migrates each active delta directory atomically as part of
the same commit. Each delta file is split per-requirement across the
partitions its requirements target. Change owners rebase from master
before archive to pick up the migrated paths; opsx-sync reads the
relocated delta spec paths automatically.

### D6 -- Direct live-spec migration with `--skip-specs` archive

This change applies the split directly to the live spec files rather than
relying on the OpenSpec change's delta specs to do so at archive time.
Rationale:

- The OpenSpec `opsx-sync` workflow expects delta specs to add/remove
  requirements in the main spec. With the live specs already split, a
  normal `openspec archive partition-session-relay-spec` would attempt to
  ADD the 94 requirements to the partition specs (which already have them)
  and fail with `already exists` errors.
- The split is a one-time refactor; the delta specs in
  `<change>/specs/` are kept as descriptive documentation of the move but
  are not applied at archive.
- Archive uses `--skip-specs` to skip the (already-applied) delta
  application. The change proposal/docs are archived as usual; the live
  specs remain in their post-split state.

This is an exceptional workflow: most changes DO apply delta specs at
archive. The exception is documented in `tasks.md` 5.3 so reviewers and
future operators know why this change does not follow the default path.

## Risks / Trade-offs

- **Migration debt on active changes (RESOLVED by atomic migration, D5):**
  Each active change owner rebase from master before archive. Tracked
  under `todos/general/31` with per-change mappings.
- **Reviewer mental-model shift:** existing reviewers know `session-relay`
  as a single spec. The 8-partition layout requires them to identify the
  right partition before reading. Mitigation: the hub file's partition
  index table is the entry point for the existing
  `openspec show session-relay` workflow.
- **Cross-partition coupling:** some requirements reference concepts in
  other partitions. The references are by requirement name, which is
  preserved; no semantic drift.
- **`--skip-specs` archive:** reviewers familiar with the default workflow
  may flag this as an exception. Mitigation: D6 documents the rationale,
  and `tasks.md` 5.3 records the actual archive command.
- **Cross-spec references in `transport-abstraction/spec.md`:** three
  references to `session-relay` updated to `transport-contracts`.
  Verified by `grep -n "session-relay"` returning zero matches in
  `transport-abstraction/spec.md` post-change.
