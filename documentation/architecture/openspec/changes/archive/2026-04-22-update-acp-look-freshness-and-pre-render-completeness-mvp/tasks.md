## 1. Spec Relock

- [x] 1.1 Update `session-relay` look contract to discriminated payload format:
  tmux lines + ACP structured entries.
- [x] 1.2 Lock deterministic replay ingestion/write order (`session/load`
  replace baseline + live append) and retention behavior.
- [x] 1.3 Lock deterministic freshness predicate order and authoritative age
  source precedence.
- [x] 1.4 Lock compatibility posture:
  - tmux shape unchanged,
  - ACP shape intentionally changed pre-MVP,
  - additive freshness fields preserved for ACP.
- [x] 1.5 Lock replace-on-first-successful-structured-load compatibility
  handoff where legacy flattened ACP snapshots are ignored until canonical
  structured baseline replacement.
- [x] 1.6 Update MCP look contract to preserve relay structured ACP payloads
  unchanged (no parsing/transforms).
- [x] 1.7 Update CLI look contract to preserve relay structured ACP payloads
  unchanged in machine output.

## 2. Implementation Follow-up (post-approval)

- [x] 2.1 Add shared ACP replay-to-structured conversion under `src/acp/`.
- [x] 2.2 Route relay ACP snapshot writes through one authoritative ingestion
  path.
- [x] 2.3 Update freshness derivation logic to match locked predicate order.
- [x] 2.4 Implement replace-on-first-successful-structured-load compatibility
  handoff with legacy flattened snapshot state ignored prior to replacement.
- [x] 2.5 Add regression tests for:
  - ACP entry ordering and retention,
  - compatibility handoff replacement,
  - freshness transitions,
  - tmux lines-vs-ACP structured discriminated response shape.

## 3. Validation

- [x] 3.1 Run `openspec validate update-acp-look-freshness-and-pre-render-completeness-mvp --strict`.
