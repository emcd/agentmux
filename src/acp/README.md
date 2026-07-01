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
  - `Invocation`: never merges (per-call boundary must be preserved).
  - Different-kind adjacency never merges.
  The 1000-entry cap is enforced after coalescence so a chatty turn's
  coalesced single entry does not advance the cap faster than a
  fragmented equivalent.

- **`append_replay_entries`** (prompt path). Non-coalescing; each
  operator submission produces its own `ReplayEntry::User` regardless
  of any preceding `User` tail. Two back-to-back prompts remain two
  distinct buffer entries — adjacent whole-delivered submissions are
  semantically distinct inputs to the language model and the operator
  aggregation principle says we only merge messages that arrive as
  streaming deltas (user prompts don't).

The split is enforced at the call boundary (`dispatch_session_update`
uses `coalesce_replay_entries_on_append`; `AcpStdioClient::prompt` uses
`append_replay_entries`) so future paths that want to ingest cannot
accidentally default to coalescing.

The `tool_call` + `tool_call_update` merge lives in
`parse_replay_entries_from_params` (parser-side accumulator keyed by
`call_id` in `pending_tool_calls`); it is orthogonal to replay-buffer
coalescence. The Pending -> Completed in-place replace is a separate,
related concern (tracked under the `todos/acp/21` future work).

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
