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

## No elapsed-time bound

ACP delivery has no turn timer. The wait resolves on
`PromptCompletion`, agent process close, dispatcher refusal,
serialization failure, or shutdown only — never on a clock. No
per-coder key and no envelope field can bound it; the delivery
contract bounds the queue behind an unready target instead of
declaring a slow turn a non-delivery.

Wedge detection is intentionally not applied to the ACP path. ACP
does no snapshot polling, so there is no settled non-prompt frame to
classify and no empty-pane mismatch to compare against. No transport
classifies `wedged` any more — Tmux stopped in
`bound-tmux-readiness-wait` and Pty's classifier was deleted with the
rest of the wedge machinery, because a settled non-prompt frame cannot
be told apart from a permission dialog or a coder working silently.
ACP has no elapsed-time bound today; the relay's
submission-timeout watchdog (when armed) bounds the supervised code's
runtime instead, but arming depends on relay-side submission-evidence
work that is not yet in this slice.

## Terminal-outcome receipt rendering

A terminal-outcome receipt (a relay/system-originated informational
turn back to the original sender for a queued message whose delivery
later resolved to `failed`, `timeout`, or `dropped_on_shutdown`; see
`src/relay/README.md` for the relay-side spawn/route/drop mechanics)
carries a per-envelope `is_receipt: bool` marker on `DeliveryEnvelope`
and is rendered by the ACP transport as its own turn with three
shape-specific behaviors:

- **Flush barrier.** A receipt is never absorbed into a peer-traffic
  flush group, and it never coalesces with surrounding peer envelopes.
  When a receipt is the head of the internal write channel the
  transport submits it as a singleton turn (no inner absorb loop runs);
  when a receipt arrives mid-batch the transport flushes the pending
  peer batch first and then submits the receipt alone on its own turn.
  The agent always observes a receipt on a turn by itself, separated
  from any peer message that may be queued beside it.
- **Zero quiet-window on ACP receivers.** The relay's
  `build_coder_envelope` zeros `quiet_window` on the envelope when the
  receipt's resolved target transport is ACP, satisfying the
  "receipt bypasses quiescence" invariant at the envelope seam. ACP
  ignores `quiet_window` today, but the construction is explicit at
  the relay so the invariant holds regardless of whether the ACP
  transport starts honoring it in a future change. Receipts addressed
  to Tmux/Pty senders keep the default async quiet-window, so the
  per-transport quiescence behavior is unchanged for those senders;
  the ACP-only zero keeps the invariant ACP-scoped.
- **Empty choice-decider sessions.** A receipt task carries
  `choice_decider_sessions: Vec::new()`. The relay therefore does not
  authorize any UI session to decide a choice raised on the receipt
  turn: if the agent does raise a `session/request_permission` while
  processing the receipt, the chooser enqueues the choice with no
  authorized deciders and the choice request remains pending until a
  terminal cancellation condition fires (relay shutdown, worker
  respawn invalidation, or session teardown). `submit_envelope_turn`
  installs a permission handler whenever `ctx.chooser` is present,
  regardless of the decider list — the receipt's empty deciders affect
  authorization, not handler creation. The handler is the
  `acp-permission-resolver` thread, not a quiet path; an agent that
  raises a permission request on a receipt turn still pays the
  resolver-thread cost and that thread remains occupied (blocked on the
  chooser's no-decider queue) until the request is cancelled. The
  agent does not receive a quick outcome; it sees the turn itself
  stall on the in-flight permission request. Receipt authors should
  write receipt bodies that prompt the agent for a textual
  acknowledgement, not a tool call.

The receipt runs through `submit_envelope_turn` like any other turn
and surfaces its terminal outcome through the same `SingleDeliveryOutcome`
channel every other delivery uses. ACP's turn wait has no elapsed-time
bound; a receipt resolves on completion, agent close, dispatcher
refusal, or shutdown — not on a
timer. Non-recursion is enforced at the relay's single terminal-
resolution spawn site; the ACP transport never has to know about it.

`submit_singleton_envelope` is the helper that encapsulates the
flush-barrier semantics for the receipt's own turn submission; it is
a thin wrapper around `flush_envelope_group` over a one-element batch
plus the post-turn respawn signal. The barrier decision (when to
isolate a receipt from peer traffic) lives one level up, in
`plan_inner_actions`, which `acp_delivery_task` calls once per head.
The plan returns a `DeliveryPlan` (peers to absorb into the in-flight
batch plus a single `BoundaryAction` — return-to-outer-loop, submit
receipt singleton, or submit raw). `execute_delivery_plan` applies
the plan against the live transport; the plan itself is pure over
its closures (`pull_next`, `is_receipt_envelope`, `is_raw_write_item`,
`should_stop`), which is what makes it testable without spinning up
the delivery task's blocking submit path. The inline
`delivery_plan_tests` module exercises the receipt and raw barrier
rules deterministically and observes each plan's queue remainder so
the test's continuation matches production.
