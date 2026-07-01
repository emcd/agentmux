# Change: Replace Pending -> Completed Invocation entry in place by call_id

## Why

A single ACP tool call\'s full lifecycle produces TWO separate buffer
entries today:

- The `tool_call` notification pushes a `ReplayEntry::Invocation`
  with `status="pending"`.
- The matching `tool_call_update` notification (carrying
  `status="completed"` and the optional result payload) appends a
  SECOND, separate `ReplayEntry::Invocation` with the same
  `call_id`.

A `look` snapshot of an ACP session mid-multi-tool-call therefore
shows each tool call twice. The existing session-relay requirement
**ACP Look Snapshot Contract** already mandates one entry per tool
call (the "Coalesce tool call and result onto one invocation entry"
scenario at session-relay/spec.md:1322); the current implementation
just doesn\'t enforce it.

Cross-call contamination is NOT a risk: `pending_tool_calls`
already correlates `tool_call` + `tool_call_update` pairs by
`call_id`, so concurrent in-flight calls do not confuse each other.
The bug is strictly within one call\'s lifecycle -- two buffer
entries per logical call.

## What Changes

- `parse_replay_entries_from_params` becomes a buffer-aware
  ingestion step. Today it returns `Vec<ReplayEntry>` and is
  followed by a buffer-lock + `coalesce_replay_entries_on_append`
  call in `dispatch_session_update`. Under /21 the parser takes
  `&mut Vec<ReplayEntry>` and processes tool-call-related
  notifications directly: it pushes Pending entries into the
  buffer and mutates existing entries in place on terminal
  updates.
- The parser-side `pending_tool_calls` HashMap grows a position
  field per pending entry. The value type becomes a new struct
  carrying `entry: ReplayEntry` AND `buffer_position: usize`. When
  `tool_call` is parsed, the parser pushes the Pending entry into
  the buffer and records `buffer_position = buffer.len() - 1`. On
  a terminal `tool_call_update` (in v1: `status="completed"`), the
  parser mutates `buffer[pending_calls[call_id].buffer_position]`
  in place to set `status="completed"` and `result = Some(payload)`
  and removes the entry from `pending_tool_calls`. The buffer\'s
  entry count does not advance on the completion.
- Helper split: the existing `coalesce_replay_entries_on_append`
  helper from /20 currently does two things -- it performs
  adjacent same-kind coalescence AND enforces the 1000-entry
  buffer cap by draining oldest entries. The /21 change decouples
  these so the cap enforcement is observable to the parser:
  - `coalesce_replay_entries_on_append(buffer, new_entries)` keeps
    its adjacency-coalesce-and-append semantics but NO LONGER
    drains.
  - A new helper
    `enforce_replay_buffer_cap_and_maintain_positions(buffer:
    &mut Vec<ReplayEntry>, pending_calls: &mut
    HashMap<String, PendingToolCall>)` owns the 1000-entry cap:
    it drains oldest entries first, decrements every recorded
    `buffer_position` by the drain count, and removes any
    `PendingToolCall` whose recorded position fell below the
    threshold (its Pending Invocation was evicted).
  The parser calls adjacent-coalesce zero-or-more times during
  its ingest pass and then calls the cap-maintain helper once at
  the end so cap drain and position maintenance happen as one
  observable operation.
- Cap-aware position maintenance is therefore atomic with cap
  enforcement: the parser\'s quiescent invariant -- that every
  remaining `pending_calls[call_id].buffer_position` points to a
  valid `Invocation` entry with matching `call_id` in the
  buffer -- holds because the cap-maintain helper is the only
  code path that evicts from the buffer front, and it does so
  atomically with the position adjustment.
- If `tool_call_update` arrives for an unknown `call_id` (e.g.,
  replay-baseline shapes that don\'t emit a preceding Pending),
  the parser falls through and pushes a single Completed
  Invocation directly. The same fallthrough applies when a known
  `tool_call_update` arrives and the corresponding Pending
  Invocation has already been evicted by the cap: the parser
  treats the update as if the Pending had never existed and
  emits a single Completed entry.
- Status vocabulary: stays at the existing `ToolCallStatus = {
  Pending, Completed }`. `Failed` and `InProgress` variants -- and
  the broader v2 patch-fields surface (`title`, `kind`, `content`,
  `locations`, `rawInput`, `rawOutput`) from the ACP v2 RFD -- are
  deliberately out of scope; they require a separate vocabulary
  change (rendering, snapshot serialization, look wire format) and
  are tracked under `todos/acp/21` Future work.

The /20 (`aggregate-acp-streaming-chunks`) helper is the wrong
mechanism for tool-call lifecycle coalescence: it merges by
buffer-position adjacency, but a `tool_call` and its matching
`tool_call_update` can be arbitrarily separated by other
notifications (agent text, cognition, other concurrent tool calls).
Replace-by-key in the parser is the right mechanism because it
correlates by `call_id` (the upstream\'s correlation token, not by
buffer arrangement).

## Impact

- Affected specs: `session-relay`. The existing **ACP Look Snapshot
  Contract** is MODIFIED: the canonical vocabulary refinement
  uses the existing status (`Pending` / `Completed`), and the
  existing "Coalesce tool call and result onto one invocation entry"
  scenario is sharpened to make the in-place per-`call_id`
  mechanism explicit (matching is by `call_id`, not by adjacency;
  mutation happens in place even when intermediate notifications
  arrive). Two new scenarios pin the contract.
- Affected code:
  - `src/acp/client.rs` -- new struct `PendingToolCall` (carries
    the cached entry plus its buffer position; replaces the
    current `ReplayEntry` HashMap value). Parser signature gains
    `&mut Vec<ReplayEntry>` and processes notifications in place
    under the caller-held buffer lock. The existing
    `coalesce_replay_entries_on_append` helper loses its drain
    step (becomes adjacency-coalesce-only); a new
    `enforce_replay_buffer_cap_and_maintain_positions` helper
    carries the cap drain and the `pending_calls` position
    update atomically.
  - `src/acp/mod.rs` -- `PendingToolCall` is `pub(in crate::acp)`;
    `ReplayEntry` keeps its current shape. The test re-export for
    the coalesce helper is renamed or supplemented with one for
    the cap-maintain helper.
  - `tests/unit/acp/replay_coalescence.rs` -- existing tests that
    asserted cap behavior inside the old coalesce helper are
    updated to call both helpers in sequence (or split into two
    tests: one for adjacent coalescence, one for cap-maintain).
  - No file outside `src/acp/` or its tests is touched.
- `ReplayEntry` keeps its current shape; the status vocabulary and
  serialization are unchanged.
- Backwards compatible. Callers that today see N+1 entries per tool
  call will see N entries per tool call (one fewer). The vocabulary
  (`kind = "invocation"`, `status`, `call_id`, `invocation`,
  `result`) is unchanged.
- Out of scope (deferred to separate OpenSpec changes): the v2
  ACP `tool_call_update` patch fields, the v2
  `tool_call_content_chunk` streaming accumulator (per
  `todos/acp/21` Future work), and the `Failed` /
  `InProgress` status variants.

## Amendment history

1. **Fix-point boundary** (RG post-proposal review). The original
   draft described the parser mutating buffer positions without
   specifying the function-boundary change. RG noted that
   `parse_replay_entries_from_params` currently returns
   `Vec<ReplayEntry>` and never touches the buffer; mutation
   therefore requires the parser to take the buffer. The amendment
   makes the buffer-aware ingestion step explicit (Tasks 1.1-1.2
   cover the signature change and the new struct) and frames the
   rest of the mechanism against that boundary.
2. **Status vocabulary scope** (same RG review). The original
   draft mentioned `failed` and `in_progress` as if they were
   existing statuses; the v1 `ToolCallStatus` enum has only
   `Pending` / `Completed`. The amendment locks the scope at v1
   (`Pending` -> `Completed`) and moves `failed` / `in_progress`
   plus the v2 patch-fields surface to a separate OpenSpec change
   that widens the vocabulary. The spec\'s vocabulary list and the
   terminal-status scenario are corrected accordingly.
3. **Cap-eviction-aware position maintenance** (RG post-amendment
   review of cc9a53f). The prior amendment specified
   `buffer_position` but did not account for the 1000-entry
   buffer cap and oldest-first eviction: positions recorded at
   parse-time can shift when the cap drains the buffer front, or
   the Pending Invocation itself can be evicted before completion.
   The amendment adds explicit cap-aware position maintenance:
   after each append-then-coalesce pass, the parser drains the
   buffer to the cap and adjusts every recorded position down by
   the drain count, removing pending entries whose positions fall
   below the threshold. A late-arriving `tool_call_update` whose
   Pending was evicted falls through to the existing replay-
   baseline affordance (pushes a single Completed entry). Two new
   tests cover the position-shift and pending-eviction cases.
4. **Helper split for observable cap enforcement** (RG post-
   amendment review of a6338ee). The cc9a53f amendment routed
   tool-call appends through the existing
   `coalesce_replay_entries_on_append` helper, which (per the
   /20 contract) drains oldest entries internally and does not
   surface an eviction count. The cap-aware position maintenance
   requires that count, atomically paired with the position
   adjustment, but a drain-inside-the-helper model cannot
   deliver it. The amendment extracts a new
   `enforce_replay_buffer_cap_and_maintain_positions(buffer,
   pending_calls)` helper that owns the 1000-entry cap AND the
   position-maintenance math in one operation; the coalesce
   helper loses its drain step (becomes pure adjacent-coalesce
   + append). The parser now calls the coalesce helper zero-or-
   more times during its ingest pass, then the cap helper once
   at the end. Existing tests on the coalesce helper are
   updated (or split) to exercise both helpers. Quiescent
   invariant tightened accordingly: every remaining
   `pending_calls[call_id].buffer_position` points to a valid
   matching Invocation (no `None` case, because evicted entries
   are removed from `pending_calls` entirely).
