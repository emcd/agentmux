## 1. Spec Relock

- [ ] 1.1 Update `session-relay` look contract to discriminated payload format:
  tmux lines + ACP structured entries.
- [ ] 1.2 Lock deterministic replay ingestion/write order (`session/load`
  replace baseline + live append) and retention behavior.
- [ ] 1.3 Lock deterministic freshness predicate order and authoritative age
  source precedence.
- [ ] 1.4 Lock compatibility posture:
  - tmux shape unchanged,
  - ACP shape intentionally changed pre-MVP,
  - additive freshness fields preserved for ACP.
- [ ] 1.5 Lock replace-on-first-successful-structured-load compatibility
  handoff from legacy flattened ACP snapshots to canonical structured ACP
  snapshot baseline.
- [ ] 1.6 Update MCP look contract to preserve relay structured ACP payloads
  unchanged (no parsing/transforms).
- [ ] 1.7 Update CLI look contract to preserve relay structured ACP payloads
  unchanged in machine output.

## 2. Implementation Follow-up (post-approval)

- [ ] 2.1 Add shared ACP replay-to-structured conversion under `src/acp/`.
- [ ] 2.2 Route relay ACP snapshot writes through one authoritative ingestion
  path.
- [ ] 2.3 Update freshness derivation logic to match locked predicate order.
- [ ] 2.4 Implement replace-on-first-successful-structured-load compatibility
  handoff for legacy flattened snapshot state.
- [ ] 2.5 Add regression tests for:
  - ACP entry ordering and retention,
  - compatibility handoff replacement,
  - freshness transitions,
  - tmux lines-vs-ACP structured discriminated response shape.

## 3. Validation

- [ ] 3.1 Run `openspec validate update-acp-look-freshness-and-pre-render-completeness-mvp --strict`.
