## 1. Implementation

- [x] 1.1 Add a coalescing helper
      `coalesce_replay_entries_on_append(buffer: &mut Vec<ReplayEntry>,
      new_entries: Vec<ReplayEntry>)` to `src/acp/client.rs` that walks
      new_entries, merges same-kind adjacency with the buffer tail, and
      applies the existing 1000-entry cap after coalescence. The helper
      preserves all line content in receive order; the cap is enforced
      last. Function is `pub(in crate::acp)`.
- [x] 1.2 Helper scope: same-kind User, Agent, Cognition, Update with
      matching `update_kind` merge by `lines: Vec<String>` extension.
      Invocation is never merged. Different-kind adjacency does not merge.
      Empty-line entries are not specially handled (the parser already
      drops them; the helper is robust if any leak through).
- [x] 1.3 Add a non-coalescing prompt-path helper
      `append_prompt_user_entry(buffer: &mut Vec<ReplayEntry>, entry:
      ReplayEntry)` to `src/acp/client.rs` that pushes the entry
      directly and enforces the cap. Function is `pub(in crate::acp)`.
      The helper exists to keep the prompt-vs-stream rule distinction
      visible at the boundary, not as a safety net for the coalescing
      helper's callers.
- [x] 1.4 Replace the call site in
      `AcpStdioClient::dispatch_session_update` so the reader-thread
      ingestion path goes through the coalescing helper. Replace the
      call site in `AcpStdioClient::prompt` so the prompt path goes
      through the non-coalescing helper. The existing
      `append_replay_entries` symbol can either be renamed and reused
      as one of the two helpers or split into both helpers; choose the
      smallest-diff path that keeps both helpers intact.

## 2. Tests

- [x] 2.1 `tests/unit/acp/replay_coalescence.rs`: coalescing helper covers
      the within-batch same-kind merge (single helper, multi-entry batch).
- [x] 2.2 `tests/unit/acp/replay_coalescence.rs`: coalescing helper covers
      the cross-batch tail extension (buffer tail User + new entry User
      merges into one User entry; the cap is enforced after).
- [x] 2.3 `tests/unit/acp/replay_coalescence.rs`: coalescing helper covers
      different-kind adjacency (no merge) and Invocation adjacency
      (no merge regardless of identity).
- [x] 2.4 `tests/unit/acp/replay_coalescence.rs`: coalescing helper covers
      `Update` discriminant-aware merging (matching `update_kind` merges;
      differing `update_kind` does not).
- [x] 2.5 `tests/unit/acp/replay_coalescence.rs`: coalescing helper covers
      cap enforcement after coalescence (a 998-entry buffer + a same-kind
      multi-line batch lands at 999 entries; a second same-kind batch
      evicts the oldest entry to keep the bound at 1000).
- [x] 2.6 `tests/unit/acp/replay_coalescence.rs`: non-coalescing prompt-
      path helper asserts that two back-to-back prompt User appends with
      no Agent/Cognition entries between them remain two `User` entries
      (the buffer tail is `User`; the second call pushes a new User
      entry, not an extension).
- [x] 2.7 `tests/unit/acp/replay_coalescence.rs`: session/load replay-
      history scenario. Construct an input vec that mimics the format
      `session/load` produces through the reader thread (multi-entry
      same-kind per turn with mixed kinds across turns, e.g.,
      `User, Agent, Agent, Cognition, User, Agent, Agent` per turn
      repeated). Run the coalescing helper across this vec against an
      empty buffer; assert that the resulting buffer has one entry per
      kind per turn (no fragment entries).
- [x] 2.8 `tests/integration/acp/lifecycle.rs`: integration scenario where
      a streamed multi-chunk assistant turn becomes one `Agent` entry
      (not N fragment entries) in a subsequent `look` response.
      (Implemented as `tests/integration/acp/look.rs::acp_look_coalesces_long_streaming_response_into_single_entry`
      which exercises the 1105-chunk scenario end-to-end.)

## 3. Documentation

- [x] 3.1 Update `src/acp/README.md` (and `src/acp/client.rs` module doc)
      to document the coalescence rule at the same place where the
      `tool_call` + `tool_call_update` merge is currently documented.
- [x] 3.2 No changes to CLI/MCP/TUI/README files; their consumers are
      coalescence-agnostic.

## 4. Follow-up commits (post-initial-implementation, in response to RG review of the implementation)

- [x] 4.1 **UserSource provenance field** on `ReplayEntry::User` to
      block cross-source coalescence (RG post-merge review of the
      implementation). The initial implementation merged `User`-tail
      + `User`-arrival regardless of origin, which would have
      silently combined a prompt-origin tail with a reader-thread
      arrival. The fix adds a `UserSource` enum
      (`PromptPath` / `ReaderThread`); the parser sets
      `ReaderThread` for `session/update`/`session/load`, the prompt
      path sets `PromptPath`; `try_merge_adjacent` checks source
      equality on the `User` case and refuses cross-source merges.
      Two regression tests pin the contract.
- [x] 4.2 **Source / external-label references in source comments**
      reworded per project development-practices guidance (RG
      non-blocking). Doc comments and test comments reference
      generic behaviour descriptions now (e.g., "reader-thread same-
      kind adjacency coalescence") instead of referencing the
      change-ID.
