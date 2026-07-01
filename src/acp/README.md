# src/acp/

Agent Client Protocol (ACP) transport: the worker that connects to an ACP
agent subprocess (via JSON-RPC over stdio), holds the shared per-target
session state, and relays sends/look requests between the upstream ACP
server and the agentmux relay.

## Replay buffer coalescence

`AcpStdioClient` maintains an in-memory replay buffer (`Vec<ReplayEntry>`)
shared between the reader thread (ingestion from the ACP server) and the
prompt path (operator submissions). The replay buffer is the source of
truth for the look snapshot; consumers (CLI, MCP, relay look handler, TUI)
read it verbatim and are intentionally coalescence-agnostic.

The buffer append step is split into two helpers:

- **`coalesce_replay_entries_on_append`** (reader-thread path). Walks
  the new entries, merging adjacent same-kind entries with the buffer
  tail by `lines` extension. Coalescence scope:
  - `User`, `Agent`, `Cognition`: adjacent same-kind entries merge by
    `lines` array extension.
  - `Update`: adjacent entries merge only when `update_kind` matches.
  - `Invocation`: never merges (per-call boundary must be preserved;
    the parser-side tool-call lifecycle replace-by-key is a separate
    mechanism).
  - Different-kind adjacency never merges.
  This helper is now **cap-free**; it does NOT drain the buffer. The
  reader-thread parser calls `coalesce_replay_entries_on_append` zero-
  or-more times during its ingest pass (one call per `params.update`
  array entry, so wire order is preserved within a notification) and
  then calls `enforce_replay_buffer_cap_and_maintain_positions` once
  at the end of the pass so cap drain and recorded-position adjustment
  happen as one atomic operation.

- **`append_replay_entries`** (prompt path). Non-coalescing; each
  operator submission produces its own `ReplayEntry::User` regardless
  of any preceding `User` tail. Two back-to-back prompts remain two
  distinct buffer entries — adjacent whole-delivered submissions are
  semantically distinct inputs to the language model and the operator
  aggregation principle says we only merge messages that arrive as
  streaming deltas (user prompts don't). After the append, this helper
  also calls `enforce_replay_buffer_cap_and_maintain_positions` so a
  prompt-path overflow cannot leave the parser-side `pending_tool_calls`
  map with stale `buffer_position` values.

The split is enforced at the call boundary (`dispatch_session_update`
uses `coalesce_replay_entries_on_append`; `AcpStdioClient::prompt` uses
`append_replay_entries`) so future paths that want to ingest cannot
accidentally default to coalescing.

## Tool-call lifecycle coalescence

`parse_replay_entries_from_params` is buffer-aware: it ingests each
`session/update` notification directly into the buffer under the
caller-held lock, and correlates `tool_call` + `tool_call_update`
notifications by `call_id` via the `pending_tool_calls` map
(`HashMap<String, PendingToolCall>`).

- On `tool_call`: a `Pending` `ReplayEntry::Invocation` is pushed into
  the buffer and the recorded `buffer_position` is stored in
  `pending_tool_calls[call_id]`.
- On `tool_call_update` for a known `call_id`: the buffer entry at the
  recorded position is mutated in place to set
  `status = ToolCallStatus::Completed` and `result = Some(payload)`.
  The pending entry is removed from the map. The buffer's entry count
  does not advance on the completion.
- On `tool_call_update` for an unknown `call_id` (replay-baseline
  shape, or a Pending evicted by the cap): a single Completed
  Invocation is pushed via the coalesce helper.

A single tool call therefore produces exactly one `ReplayEntry` in
`look` regardless of how many intermediate notifications (agent text,
cognition, other concurrent tool calls) arrive between the `tool_call`
and its `tool_call_update`.

`pending_tool_calls` is shared between the reader thread and the
prompt path (`SharedPendingToolCalls = Arc<Mutex<HashMap<...>>>`). The
prompt path locks both the replay buffer and the pending map so a
prompt-path append that trips the cap cannot evict a Pending
Invocation whose recorded position is still in the map; the
`enforce_replay_buffer_cap_and_maintain_positions` helper drains the
buffer and adjusts every recorded position (or removes pendings whose
Pending Invocations were evicted) atomically.

## Wire-order preservation

The parser processes each entry in `params.update` in array order.
Non-tool entries are appended via `coalesce_replay_entries_on_append`
immediately so they land in the buffer in the order the wire sent
them. `tool_call` entries are pushed in place (Invocations never merge
with adjacent entries). `tool_call_update` entries mutate the existing
buffer entry in place by `call_id` or push a single orphan Completed
entry. Adjacent same-kind non-tool entries within the same
notification still coalesce because each append goes through the same
coalesce helper that walks the buffer tail; the wire order is
preserved across notification boundaries too because every append
goes through the same coalesce path.

The status vocabulary stays at v1 (`Pending` / `Completed`). `Failed`
and `InProgress`, plus the v2 ACP `tool_call_update` patch fields
(`title`, `kind`, `content`, `locations`, `rawInput`, `rawOutput`) and
the v2 `tool_call_content_chunk` streaming accumulator, are deferred
to a separate OpenSpec change that widens the vocabulary; they are
out of scope for this design and should land in a follow-up proposal
that covers the rendering, snapshot serialization, and look wire
format changes the broader status vocabulary requires.

## Wire-level streaming

ACP server streaming comes through `session/update` notifications
carrying `agent_message_chunk`, `agent_thought_chunk`, and
`user_message_chunk` payloads. The parser collapses each notification's
per-chunk content into a `ReplayEntry` whose `lines: Vec<String>`
holds the chunk's text. The coalescing helper then absorbs adjacent
same-kind chunks into one entry per turn.

`session/load` (worker reconnect / resume) replays session history
through the same reader-thread path, so coalescence also covers the
reconnect-coalescence invariant. Look snapshots after a reconnect
hold coherent turns rather than fragments of the same turn.
