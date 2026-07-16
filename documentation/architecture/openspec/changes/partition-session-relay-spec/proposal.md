# Change: Partition session-relay specification into capability-scoped specs

## Why

`openspec/specs/session-relay/spec.md` is 3771 lines containing 94 normative
requirements spanning addressing, delivery, transport, authorization, look,
stream events, choice, bundle lifecycle, and environment variables. The length
actively hides defects: the `issues/relay/53` near-miss (async-only submit at
L338-344 vs synchronous-propagation clause at L3347-3351) survived review
because nothing forced a cross-check between clauses ~3000 lines apart. Long
single-file specs accumulate redundancy and latent contradictions.

Per the dispatch (`todos/general/31`, allocated to 0.10.0 housekeeping /
tech-debt), the spec is partitioned by capability into 8 sibling specs under
`openspec/specs/`. Each requirement moves verbatim: byte-for-byte text
preservation across all 94 blocks (sha256-verified). The original
`session-relay/spec.md` becomes a hub file: a partition index + archive-order
safety notes + a single new hub requirement documenting the split.

## What Changes

- **Remove** all 94 requirements from `openspec/specs/session-relay/spec.md`.
- **Add** 1 hub requirement to `openspec/specs/session-relay/spec.md`:
  **`Session-Relay Specification Partition Index`** -- documents the
  partition structure and the path-migration obligation for active OpenSpec
  changes (covering both `## MODIFIED Requirements` and `## ADDED
  Requirements` deltas).
- **Add** 8 sibling specs under `openspec/specs/`, one per partition
  capability:
  - `addressing-routing` (13 requirements) -- canonical IDs, namespace
    semantics, target resolution, list payloads, raww target resolution.
  - `delivery-quiescence` (7 requirements) -- send envelope, async queue
    lifecycle, terminal outcomes, ack semantics, relay/53 receipt.
  - `transport-contracts` (23 requirements) -- per-transport execution
    contracts (tmux, ACP, raww execution/input, Pty timeouts/wedge,
    ACP lifecycle mechanics, ACP transport error, transport capability,
    ACP/transport timeouts, copy-mode-transparent injection). Cross-cutting
    raww target resolution and authorization mapping moved OUT to
    `addressing-routing` and `authorization-scope` to keep transport-contracts
    coherent. ACP Look Snapshot and Freshness moved IN to
    `look-and-stream-events` to keep cross-transport Look contracts together.
  - `authorization-scope` (13 requirements) -- policy presets, auth
    vocabulary and evaluation, scope controls, uniform cross-bundle auth,
    UI sender validation, list-sessions scope, raww authorization mapping.
  - `look-and-stream-events` (10 requirements) -- Look operation
    (transport-agnostic + per-transport ACP Look contracts), persistent
    client streams, Hello, stream event contract.
  - `choice-decisions` (14 requirements) -- choice/decision envelope, queue
    lifecycle, operator classes.
  - `bundle-lifecycle` (10 requirements) -- reconciliation, bundle
    up/down, startup health, file watching.
  - `environment-variables` (4 requirements) -- coder/bundle/session env
    variable precedence, container-injected overrides.

The `runtime-api` partition is a future capability owned by the active
`embeddable-runtime-api` change. Its 10 ADDED requirements will create
`openspec/specs/runtime-api/spec.md` when that change archives; this
partition change does not pre-create the live spec.
- **Migrate** 7 active OpenSpec changes' delta spec paths from
  `<change>/specs/session-relay/spec.md` to `<change>/specs/<partition>/
  spec.md`, atomically as part of this change. Each delta file is split
  per-requirement across the partitions its requirements target.
- **Update** cross-spec references in `openspec/specs/transport-abstraction/
  spec.md` (lines 489-490 and 505-506) from `session-relay` to
  `transport-contracts` to match the new partition locations of Tmux Prime
  Timeout, Tmux Wedged State Detection, and Copy-Mode-Transparent Injection.

All 94 moved requirements are reproduced **byte-for-byte** from the
pre-split text (verified via sha256 in
`.auxiliary/scribbles/verify_split_v3.py`). The requirement title, body,
and `#### Scenario:` blocks are unchanged.

## Impact

- **Affected specs:**
  - `session-relay` -- REMOVED 94, ADDED 1 (hub index).
  - `addressing-routing`, `delivery-quiescence`, `transport-contracts`,
    `authorization-scope`, `look-and-stream-events`, `bundle-lifecycle`,
    `environment-variables` -- ADDED their assigned requirements (verbatim).
  - `runtime-api` -- not affected; the live spec is created on
    `embeddable-runtime-api` archive.
  - `transport-abstraction` -- cross-references updated (3 lines).
- **Affected code:** none. This is a spec-only refactor; no `src/` changes.
  Implementation is gated by separate per-partition changes if/when behavior
  changes are needed.
- **Affected active OpenSpec changes** (delta spec paths migrated as part of
  this change):
  - `add-container-sandboxing` -- 4 ADDED requirements split across
    `addressing-routing` and `environment-variables`.
  - `add-do-action-tool` -- 4 ADDED requirements split across
    `authorization-scope` and `transport-contracts`.
  - `add-pty-transport` -- 4 MODIFIED + 3 ADDED requirements split across
    `addressing-routing` and `transport-contracts`.
  - `add-about-surface-and-description-fields` -- 4 ADDED requirements split
    across `addressing-routing`, `transport-contracts`, and
    `authorization-scope`.
  - `deliver-async-terminal-outcomes` (relay/53) -- 1 ADDED + 2 MODIFIED
    requirements split across `delivery-quiescence` and `transport-contracts`.
  - `embeddable-runtime-api` -- delta spec path migrated from
    `specs/session-relay/spec.md` to `specs/runtime-api/spec.md`. The
    live `runtime-api` spec is not pre-created; it is created when
    `embeddable-runtime-api` archives (opsx-sync creates the spec on
    ADD).
  - `add-e2e-test-harness` -- 1 MODIFIED requirement placed in
    `bundle-lifecycle`.
- **Archive workflow:** this change applies the split directly to live
  specs rather than relying on the delta specs to do so at archive time.
  The delta specs in `<change>/specs/` are kept as descriptive documentation
  of the move. Archive uses `--skip-specs` to skip the (already-applied)
  delta application. See `design.md` and `tasks.md` for the workflow.

## Allocation

Allocated to 0.10.0 (housekeeping / tech-debt, not urgent -- no release is
waiting on this).
