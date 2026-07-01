## 1. Implementation

- [ ] 1.1 Replace the parser-side `pending_tool_calls` HashMap value
      from `ReplayEntry` to a new struct `PendingToolCall` carrying
      both the cached entry and the recorded buffer position:
      `pub(in crate::acp) struct PendingToolCall { pub entry:
      ReplayEntry, pub buffer_position: usize }`. The HashMap is
      keyed by `call_id` as before; only the value type changes.
- [ ] 1.2 Change `parse_replay_entries_from_params` to take both
      `pending_calls: &mut HashMap<String, PendingToolCall>` and
      `buffer: &mut Vec<ReplayEntry>`. The parser ingests directly
      into the buffer (under the caller-held lock); the lock + coalesce
      dance that `dispatch_session_update` did today goes away for
      tool-call-related notifications.
- [ ] 1.3 Split the existing `coalesce_replay_entries_on_append`
      helper. Drop its 1000-entry cap drain; the helper becomes
      pure adjacency-coalesce-and-append. Add the new helper
      `enforce_replay_buffer_cap_and_maintain_positions(buffer:
      &mut Vec<ReplayEntry>, pending_calls: &mut HashMap<String,
      PendingToolCall>)` which owns the 1000-entry cap: drains
      oldest entries first, decrements every recorded
      `buffer_position` by the drain count, and removes any
      `PendingToolCall` whose recorded position fell below the
      threshold (its Pending Invocation was evicted).
- [ ] 1.4 For each `tool_call` notification in the parser: push the
      Pending entry into the buffer via the now-cap-free
      `coalesce_replay_entries_on_append` helper (Invocations do
      not coalesce with anything per /20, so the helper simply
      appends), and record `buffer_position = buffer.len() - 1`
      in `pending_calls[call_id]`.
- [ ] 1.5 For each `tool_call_update` notification in the parser:
      if `call_id` is in `pending_calls`, mutate
      `buffer[pending_calls[call_id].buffer_position]` in place to
      set `status = Completed` and `result = Some(payload)`, then
      remove the entry from `pending_calls`. The buffer\'s entry
      count does not advance.
- [ ] 1.6 For each `tool_call_update` notification whose `call_id`
      is NOT in `pending_calls` (replay-baseline shape, or Pending
      evicted by the cap): push a single Completed Invocation via
      `coalesce_replay_entries_on_append`. Preserves the existing
      replay-baseline affordance and the cap-eviction-fallthrough
      behavior.
- [ ] 1.7 After the parser\'s ingest pass (all `tool_call`,
      `tool_call_update`, and non-tool-call entries processed),
      call `enforce_replay_buffer_cap_and_maintain_positions` once.
      This makes cap drain and position maintenance observable as
      one atomic operation. The parser\'s quiescent invariant --
      every remaining `pending_calls[call_id].buffer_position`
      points to a valid `Invocation` entry with matching `call_id`
      in the buffer -- holds immediately after this call (and at
      every quiescent point between parser calls).
- [ ] 1.8 Update `dispatch_session_update` to acquire the
      replay_buffer lock and call the parser under it; remove the
      now-redundant `coalesce_replay_entries_on_append` call in
      this path (the parser owns the ingest pipeline).
- [ ] 1.9 Update `tests/unit/acp/replay_coalescence.rs`: existing
      tests that asserted cap behavior via the old
      coalesce-and-cap-combined helper are updated to exercise the
      split. Specifically, `cap_is_enforced_after_coalescence` and
      `coalescence_reduces_entry_count_before_cap_check` now call
      the coalesce helper followed by
      `enforce_replay_buffer_cap_and_maintain_positions`; the
      end-state assertions are unchanged.

## 2. Tests

- [ ] 2.1 Unit test (parser): `tool_call(A)` then `tool_call_update(A)`
      with terminal `status="completed"`. Buffer holds exactly one
      `ReplayEntry::Invocation` with `status="completed"` and the
      result payload; no second entry appears.
- [ ] 2.2 Unit test (parser): two tool_calls with out-of-order updates
      (`tool_call(A)`, `tool_call(B)`, `tool_call_update(B)`,
      `tool_call_update(A)`). Buffer holds two Invocation entries,
      each carrying its own result payload -- no cross-contamination.
- [ ] 2.3 Unit test (parser): a terminal `tool_call_update(X)` with no
      prior `tool_call(X)`. Buffer holds one Invocation entry with
      `status="completed"` and the result payload -- the
      replay-baseline affordance.
- [ ] 2.4 Unit test (parser + cap-maintain helper): cap-eviction
      position shift. Pre-fill the buffer with 995 distinct `Update`
      entries (each carrying a unique `update_kind`, so coalescence
      does not absorb them). Invoke the parser with a `tool_call`
      notification; record the resulting
      `pending_calls[call_id].buffer_position == 995`. Then invoke
      the parser with five distinct `Update` entries; this trips
      the cap (995 + 1 + 5 = 1001; the cap-maintain helper drains 1
      and shifts the recorded position to 994). Verify
      `pending_calls[call_id].buffer_position == 994` and that
      `buffer[994]` is still the same Pending Invocation (matched
      by `call_id`). A subsequent
      `tool_call_update(call_id, status="completed")` mutates
      `buffer[994]` in place to `status="completed"` and removes
      the entry from `pending_calls`.
- [ ] 2.5 Unit test (parser + cap-maintain helper): Pending evicted
      before completion. Empty buffer; push a `tool_call` (Pending
      at position 0); push 1001 distinct `Update` entries in one
      parser invocation so the cap-maintain helper drains 1 and
      removes the Pending. Verify `pending_calls` no longer
      references the `call_id`. Push a terminal
      `tool_call_update(call_id)` for the same call_id; the parser
      falls through to the replay-baseline path (Task 1.6), so the
      buffer ends up with one new Invocation entry carrying the
      result payload (and `pending_calls` remains empty).
- [ ] 2.6 Integration test in `tests/integration/acp/`: an ACP
      session streaming one tool call followed by its completion
      emits one coalesced `Invocation` entry in `look`.

## 3. Documentation

- [ ] 3.1 Update `parse_replay_entries_from_params` doc-comment in
      `src/acp/client.rs` to document the new `PendingToolCall`
      shape, the position-tracking invariant, and the
      enforcement-capability boundary between the coalesce helper
      and the cap-maintain helper.
- [ ] 3.2 Update `coalesce_replay_entries_on_append` doc-comment
      to call out the explicit loss of the drain step (moved to
      `enforce_replay_buffer_cap_and_maintain_positions`).
- [ ] 3.3 No README updates; `src/acp/README.md` already points
      readers at the parser-side `tool_call` + `tool_call_update`
      merge and the helper split.

## 4. Out of scope (deferred to separate OpenSpec changes)

- The v2 ACP `tool_call_update` patch fields (`title`, `kind`,
  `content`, `locations`, `rawInput`, `rawOutput`) and the v2
  `tool_call_content_chunk` streaming accumulator. Both require a
  `ToolCallStatus` vocabulary expansion and the corresponding
  snapshot serialization / look wire format / rendering changes.
  Per `todos/acp/21` Future work; a separate OpenSpec change is
  the right vehicle.
- The `Failed` and `InProgress` status variants. Same expansion
  scope: rendering, snapshot serialization, look wire format, and
  the per-status behavior (when does `Failed` get emitted; what
  shape does `InProgress` carry).
