## 0. Sequencing against the in-flight mailbox change

This change SYNCS FIRST. `redesign-mailbox-delivery-protocol`'s migration is a
prerequisite of THIS CHANGE'S ARCHIVE, not of its sync.

- [x] 0.1 Sync this change before Backend migrates. The migration cannot be
      committed earlier: `verify-openspec-deltas.py` runs as a pre-commit hook
      on any changed delta path and rejects a MODIFIED delta whose live
      counterpart is absent, and live `raww` does not exist until this syncs.
      Requiring the migration first is a cycle, and the only exits from it are
      bypassing the hook or this ordering
- [x] 0.2 Confirm in the sync packet that the ADDED `Relay raww transport
      behavior` is a content-preserving move — byte-identical to the live
      `transport-contracts` text. This is what makes the ordering safe:
      Backend's edits exist only in their delta, never in the text this change
      moves, so syncing first cannot drop them. Their delta then replaces the
      requirement in `raww` with their version
- [x] 0.3 Backend migrates the `Relay raww transport behavior` section from
      `specs/transport-contracts/` to `specs/raww/` and repoints its `tasks.md`
      3.11 citation, reviewed before their own sync and archive. Per
      `session-relay`'s partition index requirement, which obliges migration
      before the affected change archives
- [x] 0.4 Backend repoints ONE citation in their MODIFIED `Transport Interface
      Contract` delta: the passage naming `transport-contracts`'
      `Relay raww transport behavior`. The other `transport-contracts`
      reference in that requirement is the prompt-readiness predicate, which
      this change does not touch and which must be left alone
- [x] 0.5 This change SHALL NOT archive until 0.3 and 0.4 are reviewed and
      landed

## 1. Create the capability

- [x] 1.1 Create `specs/raww/spec.md` with a Purpose line naming it the
      relay-side semantic contract for the `raww` verb, and the six ADDED
      requirements in the delta's order: operation contract, target resolution
      and bundle boundary, authorization mapping, transport behavior, response
      contract, input bounds

## 2. Sync the source capabilities

- [x] 2.1 Sync `transport-contracts` — the four raww requirements are removed
- [x] 2.2 Sync `addressing-routing` — raww target resolution is removed; the
      general addressing grammar and session type taxonomy are untouched
- [x] 2.3 Sync `authorization-scope` — the raww authorization mapping is
      removed, and `Cross-bundle operation denied under home scope` no longer
      enumerates verbs
- [x] 2.4 Sync `relay-routing-layer` — `Authorization Stage` loses the two
      raww-specific scenarios and the `can_be_written` note, and keeps
      `Requester authorized in home namespace for cross-bundle Raww` alongside
      its send and list siblings
- [x] 2.5 Sync `session-relay` — the partition index reads nine partitions
- [x] 2.6 Sync `transport-abstraction` — `Transport Interface Contract` cites
      the `raww` capability for the raww operation contract. This repairs live
      at the moment this change breaks it. The prompt-readiness reference in
      the same requirement is untouched and must stay pointing at
      `transport-contracts`

## 3. Direct edits to the session-relay hub (not delta-governed)

- [x] 3.1 Purpose preamble: "partitioned into 8 capability-scoped sibling
      specs" becomes nine
- [x] 3.2 The `grep -h '^### Requirement'` command in the preamble gains
      `raww` in its brace expansion, so the live-total recipe stays correct
- [x] 3.3 `## Partitions` table gains a `Raww` row naming
      `openspec/specs/raww/spec.md`
- [x] 3.4 `## Partitions` table: the Addressing & Routing row drops "raww
      target resolution", the Transport Contracts row drops "raww" from its
      per-transport list, and the Authorization & Scope row's "per-operation
      authorization mappings" is checked for accuracy now that raww's has left

## 4. Verify

- [x] 4.1 `scripts/verify-openspec-deltas.py extract-raww-verb-capability`
      reports zero errors and the expected drops; re-run immediately before
      sync in case a live spec moved underneath a delta
- [x] 4.2 `openspec validate --all --strict` passes with one more capability
      than before
- [x] 4.3 Diff each requirement in `specs/raww/spec.md` against the source it
      came from; differences must be only the deliberate consolidations
- [x] 4.4 Confirm no raww requirement title survives in `transport-contracts`,
      `addressing-routing`, or `authorization-scope`
- [x] 4.5 Confirm the thirteen surface requirements in `mcp-tool-surface`,
      `cli-surface` and `tui-surface` are untouched
- [x] 4.6 Confirm the general rules that merely enumerate raww still do so,
      in the routing resolution stage, the operation body contract, and
      cross-relay target classification
- [x] 4.7 Confirm exactly one scenario in the corpus asserts that cross-bundle
      raww under `all` routes and delivers, and exactly one asserts that
      `home` or narrower is denied
- [x] 4.8 Sweep prose for citations naming `transport-contracts`,
      `addressing-routing` or `authorization-scope` as the authority for a raww
      requirement, in specs and in `src/` — a dangling citation is found
      backwards, from the prose to what this change deletes, and the retention
      script does not run that sweep

## 5. Close out

- [x] 5.1 Mark `todos/openspec/5` tasks complete through the raww half
- [x] 5.2 Record the evaluation: whether the boundary held, what had to be
      judged rather than ruled, and whether `send` should follow
      (recorded as notebook reviews/13)
- [x] 5.3 Confirm the send-specific duplication in `authorization-scope`
      (`todos/openspec/12`) is still tracked and was deliberately left alone
