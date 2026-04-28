## 1. Contract Design

- [ ] 1.1 Add relay `raww` operation contract and same-bundle selector semantics.
- [ ] 1.2 Lock canonical validation/error taxonomy:
  - `validation_unknown_target`
  - `validation_cross_bundle_unsupported`
  - `validation_invalid_params`
- [ ] 1.3 Lock deterministic success schema and accepted-phase details.

## 2. Authorization Contract

- [ ] 2.1 Add policy control `raww` with allowed scopes `none|self|all:home`.
- [ ] 2.2 Lock denial capability label `raww.write` and canonical
      `authorization_forbidden` minimum details requirements.

## 3. Transport Contract

- [ ] 3.1 Lock tmux raww behavior (literal text + optional Enter).
- [ ] 3.2 Lock ACP raww behavior on existing shared worker `session/prompt`
      path with dispatch-boundary acceptance semantics.
- [ ] 3.3 Lock unsupported target-class behavior (UI targets rejected with
      `validation_invalid_params`).

## 4. Surface Contracts

- [ ] 4.1 Add MCP `raww` tool request/response contract and association-derived
      sender authority lock.
- [x] 4.2 Add CLI `agentmux raww` contract including `--no-enter` opt-out.
- [x] 4.3 Add TUI raww dispatch contract against relay raww operation.

## 5. Implementation Follow-up (post-approval)

- [ ] 5.1 Implement relay raww request handling and policy evaluation.
- [ ] 5.2 Implement policy parsing/validation support for `raww` control.
- [ ] 5.3 Implement MCP raww tool wiring + validation.
- [x] 5.4 Implement CLI raww command wiring + JSON output mapping.
- [x] 5.5 Implement TUI raww dispatch integration.
- [ ] 5.6 Add unit/integration coverage for taxonomy, authorization, target
      classes, payload bounds, and ACP/tmux acceptance details.

## 6. Validation

- [x] 6.1 Run `openspec validate add-raww-direct-write-surface-mvp --strict`.
