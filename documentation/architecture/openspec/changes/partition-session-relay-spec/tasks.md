## 1. Spec partition

- [x] 1.1 Create 8 sibling partition specs under
      `openspec/specs/<partition>/spec.md` with verbatim requirement blocks.
- [x] 1.2 Convert `openspec/specs/session-relay/spec.md` to a hub: partition
      index + archive-order notes (covering ADDED + MODIFIED for relay/53)
      + 1 new hub requirement.
- [x] 1.3 Apply transport-contracts refinements (move 2 raww reqs out,
      move 2 ACP Look reqs in) per BE review feedback.
- [x] 1.4 (Withdrawn -- runtime-api is a future capability owned by
      `embeddable-runtime-api`. The live spec is created when that
      change archives, not by this partition change. Per BE BLOCKER 1
      re-review.)
- [x] 1.5 Verify all 94 requirement blocks byte-for-byte (sha256 in
      `.auxiliary/scribbles/verify_split_v3.py`).
- [x] 1.6 Run `openspec validate --all --strict`.

## 2. Cross-spec reference updates

- [x] 2.1 Update `openspec/specs/transport-abstraction/spec.md` lines
      489-490 from `session-relay` to `transport-contracts` (Tmux Prime
      Timeout + Tmux Wedged State Detection references).
- [x] 2.2 Update `openspec/specs/transport-abstraction/spec.md` lines
      505-506 from `session-relay` to `transport-contracts` (Copy-Mode-
      Transparent Injection reference).

## 3. Active OpenSpec change delta spec path migration (atomic)

Each active change's `specs/session-relay/spec.md` is split into
per-partition files at `<change>/specs/<partition>/spec.md` based on the
requirement target. Per-change mappings in `agentmux:todos/general/31`.

- [x] 3.1 `add-container-sandboxing`: 4 ADDED across `addressing-routing`
      (3) + `environment-variables` (1).
- [x] 3.2 `add-do-action-tool`: 4 ADDED across `authorization-scope` (2)
      + `transport-contracts` (2).
- [x] 3.3 `add-pty-transport`: 4 MODIFIED + 3 ADDED across
      `addressing-routing` (2 MOD) + `transport-contracts` (2 MOD + 3 ADD).
- [x] 3.4 `add-about-surface-and-description-fields`: 4 ADDED across
      `addressing-routing` (1) + `transport-contracts` (2) +
      `authorization-scope` (1).
- [x] 3.5 `deliver-async-terminal-outcomes` (relay/53): 1 ADDED + 2 MODIFIED
      across `delivery-quiescence` (1 ADD + 1 MOD) + `transport-contracts`
      (1 MOD).
- [x] 3.6 `embeddable-runtime-api`: delta spec path migrated from
      `specs/session-relay/spec.md` to `specs/runtime-api/spec.md`. The
      live `runtime-api` spec is not pre-created; opsx-sync creates it
      on archive.
- [x] 3.7 `add-e2e-test-harness`: 1 MODIFIED into `bundle-lifecycle`.
- [x] 3.8 Update `agentmux:todos/general/31` with the 7-change mapping
      for owner-side rebasing before archive.

## 4. Tracking + documentation

- [x] 4.1 Update `agentmux:todos/general/31` with the 7-change mapping
      and partition decisions. (`runtime-api` is no longer a partition
      created by this change -- it is a future capability owned by
      `embeddable-runtime-api`.)
- [x] 4.2 Note relay/53 archive-order relocations (ADDED
      Asynchronous Terminal-Outcome Receipt + 2 MODIFIED) in the
      session-relay hub.
- [x] 4.3 Update hub normative rule to cover `## ADDED Requirements`
      deltas (not just MODIFIED).
- [x] 4.4 Verify format fixes (missing backtick in hub delta, EOF blank
      lines).

## 5. Review and merge

- [x] 5.1 BE (AuxBE) initial review at `cd3c206` -- 2 BLOCKERs + 2
      MEDIUMs + 2 LOWs, all addressed in this amendment.
- [ ] 5.2 BE re-review (post-amendment commit).
- [ ] 5.3 Archive the change with `--skip-specs`:
      `openspec archive partition-session-relay-spec -y --skip-specs`
      (live specs already have the split applied; deltas in
      `<change>/specs/` are descriptive documentation only).
