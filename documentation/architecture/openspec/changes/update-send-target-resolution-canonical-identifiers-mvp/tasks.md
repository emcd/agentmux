## 1. Contract Relock

- [ ] 1.1 Relock relay send explicit-target semantics to canonical identifiers
  only.
- [ ] 1.2 Remove configured name/display-name alias routing scenarios from
  send-path specs.
- [ ] 1.3 Relock send-path reject code for non-canonical/unknown explicit
  targets to `validation_unknown_target`.
- [ ] 1.4 Remove send-path `validation_ambiguous_recipient` alias semantics.
- [ ] 1.5 Lock validation-code unification on `validation_unknown_target` for
  unknown/non-canonical explicit target tokens.
- [ ] 1.6 Lock deterministic overlap precedence:
  bundle member `session_id` wins over UI session id on exact-token overlap.

## 2. Surface Consistency

- [ ] 2.1 Update `session-relay` send-target requirements and scenarios.
- [ ] 2.2 Update `mcp-tool-surface` send target + validation scenarios.
- [ ] 2.3 Update `cli-surface` send target semantics and migration wording.
- [ ] 2.4 Update `tui-surface` recipient submission contract to canonical ids.

## 3. Implementation Follow-up (post-approval)

- [ ] 3.1 Update relay explicit target resolution implementation.
- [ ] 3.2 Update CLI/MCP/TUI help text or parameter documentation that implies
  display-name targeting.
- [ ] 3.3 Add/adjust tests for:
  - name alias no longer routable,
  - canonical unknown-target rejection code,
  - overlap precedence behavior.

## 4. Validation

- [ ] 4.1 Run `openspec validate update-send-target-resolution-canonical-identifiers-mvp --strict`.
