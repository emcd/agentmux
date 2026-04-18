## 1. Contract Relock

- [x] 1.1 Relock relay send explicit-target semantics to canonical identifiers
  only.
- [x] 1.2 Remove configured name/display-name alias routing scenarios from
  send-path specs.
- [x] 1.3 Relock send-path reject code for non-canonical/unknown explicit
  targets to `validation_unknown_target`.
- [x] 1.4 Remove send-path `validation_ambiguous_recipient` alias semantics.
- [x] 1.5 Lock validation-code unification on `validation_unknown_target` for
  unknown/non-canonical explicit target tokens.
- [ ] 1.6 Lock deterministic overlap precedence:
  bundle member `session_id` wins over UI session id on exact-token overlap.

## 2. Surface Consistency

- [x] 2.1 Update `session-relay` send-target requirements and scenarios.
- [x] 2.2 Update `mcp-tool-surface` send target + validation scenarios.
- [x] 2.3 Update `cli-surface` send target semantics and migration wording.
- [x] 2.4 Update `tui-surface` recipient submission contract to canonical ids.

## 3. Implementation Follow-up (post-approval)

- [x] 3.1 Update relay explicit target resolution implementation.
- [x] 3.2 Update CLI/MCP/TUI help text or parameter documentation that implies
  display-name targeting.
- [x] 3.3 Add/adjust tests for name alias no longer routable.
- [x] 3.4 Add/adjust tests for canonical unknown-target rejection code.
- [ ] 3.5 Add/adjust tests for overlap precedence behavior.

## 4. Validation

- [x] 4.1 Run `openspec validate update-send-target-resolution-canonical-identifiers-mvp --strict`.
