# Change: Aggregate ACP streaming chunks into complete turns

## Why

ACP streaming responses arrive as a sequence of `session/update` notifications
carrying per-chunk `agent_message_chunk`, `agent_thought_chunk`, and
`user_message_chunk` payloads. The current replay buffer write path in
`AcpStdioClient` appends each chunk as its own `ReplayEntry`, with no
within-buffer aggregation step. `look` then surfaces the buffer verbatim, so a
single assistant turn renders as many fragment `kind="agent"` entries (one per
chunk) rather than one coherent turn.

This is a quality bug (operators see fragments where they expect coherent turns)
and a structural mismatch (the canonical ACP vocabulary already carries
`lines: string[]` per entry; the chunk-per-entry path saturates that field with
single-line payloads rather than treating it as the per-turn content array it
was designed for).

## What Changes

- ACP transport coalesces adjacent same-kind replay entries into one entry with
  a merged `lines` array on the in-memory snapshot buffer. Coalescence happens
  on the write path (at the buffer-append step), not on the read path. No
  consumer-side change is required: CLI, MCP, relay look handler, TUI
  snapshot rendering, and the debug TUI binary (`agentmux_acp`) all keep their
  current request/response model.
- Coalescence scope is `User`, `Agent`, `Cognition`, and `Update` (only with
  matching `update_kind`). `Invocation` is excluded: an `Invocation` entry
  represents a single upstream-issued tool call and its result, already
  coalesced by the existing `tool_call` + `tool_call_update` mechanism. Two
  consecutive `Invocation` entries are two distinct tool calls and MUST NOT
  merge.
- Coalescence applies ONLY to ingestion from the ACP reader thread —
  `session/update` notifications and the `session/load` replay-history path,
  which both route through `dispatch_session_update` -> `append_replay_entries`.
  Outgoing prompt-path `User` entries (added by `AcpStdioClient::prompt`,
  which appends the operator's submitted prompt to the buffer immediately)
  preserve their entry boundary regardless of any preceding `User` tail.
  Each prompt is one `User` entry; the prompt path uses a non-coalescing
  append. Rationale: each prompt is a distinct operator submission;
  conflation would erase the per-submission boundary operators rely on for
  cancel/follow-up semantics and would conflate two distinct agent
  round-trips into one. The implementation provides two helpers in
  `src/acp/client.rs` — a coalescing helper for the reader-thread path and
  a non-coalescing append for the prompt path — both `pub(in crate::acp)`.
- Coalescence covers both ingestion sources the reader thread serves:
  `session/update` (live streaming) and `session/load` replay history
  (worker reconnect / resume). The two paths share the
  `dispatch_session_update` -> append call site, so the same coalescence
  rule applies to both. Operators reconnecting to a chatty session see
  coherent turns rather than fragments on first `look` after reconnect.
- Coalescence covers both directions of adjacency:
  - **within-notification**: when one `session/update` notification carries
    multiple entries of the same kind (the wire payload may carry a JSON
    array under `params.update`), the coalescing helper collapses them as
    part of its within-batch walk. `parse_replay_entries_from_params` is
    unchanged and continues to return a `Vec<ReplayEntry>` per
    notification;
  - **across-notification (tail extension)**: when the last entry in the
    buffer and the first entry of the new ingestion batch are the same kind,
    the new entry's `lines` are appended to the existing entry's `lines`
    instead of pushing a new entry.
- The look snapshot contract is unchanged. Each entry's `lines: string[]`
  already supports arbitrary length; coalescence only changes how many
  entries the buffer holds, not the wire vocabulary or the windowing math.
- The buffer cap (1000) is enforced after coalescence. Coalescence reduces
  the entry count of long assistant turns, which extends the buffer's
  effective temporal reach for chatty sessions; the cap is preserved as a
  bound on entry count.

## Impact

- Affected specs: `acp-client`. (`session-relay`'s **ACP Look Snapshot
  Contract** requirement is unchanged: it specifies vocabulary and ordering,
  not per-entry line count, so adding coalescence requires no delta there.
  `transport-abstraction` is also unchanged: no new contract fields are
  added; the `DeliveryEnvelope.prime_timeout_ms` field introduced by
  `tmux-wedge-detection` is unrelated.)
- Affected code:
  - `src/acp/client.rs` — split the existing `append_replay_entries` into
    two helpers, both `pub(in crate::acp)`:
    - a coalescing helper used by the reader-thread ingestion path
      (`dispatch_session_update`), which walks the new entries, merges
      same-kind adjacency against the buffer tail, and applies the
      1000-entry cap after coalescence;
    - a non-coalescing append used by the prompt path
      (`AcpStdioClient::prompt`) that retains today's behavior of pushing
      the new `User` entry directly and then enforcing the cap.
    The `parse_replay_entries_from_params` helper is NOT changed (the
    per-notification multi-entry path is already aggregated into a single
    `Vec<ReplayEntry>` before reaching the append call, so within-
    notification collapse happens at append time over that vector).
  - `src/acp/mod.rs` — no changes; `ReplayEntry` keeps its current shape.
  - `src/acp/render.rs` — no changes; `replay_entries_to_snapshot_entries`
    is a 1:1 clone and is coalescence-agnostic.
  - `src/acp/transport.rs` — no changes; `derive_acp_look_snapshot` reads
    the buffer and applies windowing math, both of which work over a buffer
    that already has the new shape.
  - `src/relay/handlers/look.rs` — no changes; transport-neutral today,
    stays neutral.
  - `src/commands/look.rs`, `src/mcp/server/handlers/look.rs`,
    `src/tui/render/interaction.rs`, `src/bin/agentmux_acp.rs` — no changes;
    per-entry-agnostic renderers.
- Backwards compatible. Existing scenarios that assert "one notification →
  one entry" continue to hold (one notification with one entry is a
  vacuous-coalescence case). Existing scenarios asserting "look returns the
  full transcript" continue to hold (coalescence preserves all line content;
  it only reduces entry count). No wire-format change, no CLI/MCP flag
  change, no configuration key change.

## Amendment history

1. **Outgoing prompt User entries preserve boundary** (RG review feedback).
   The original draft scoped coalescence by entry-kind only, which would have
   silently extended a `User` tail when the operator submits two prompts in
   rapid succession (no agent response between them). The amendment makes
   the rule scope explicit: coalescence applies to ingestion from the ACP
   reader thread only (`session/update` + `session/load`); the prompt path
   uses a separate non-coalescing append so each submitted prompt remains
   its own `User` entry. The implementation provides two helpers in
   `src/acp/client.rs`. Spec delta adds a "prompt-path User appends
   preserve their entry boundary" scenario.
2. **`session/load` replay-history path is normative and tested**
   (RG review feedback). The original draft described only live
   `session/update` ingestion in the affected scenarios; it is now
   explicit that `session/load` replay-history updates route through the
   same reader-thread ingestion path (so coalescence covers reconnect and
   resume), and a dedicated scenario + unit test pin that behavior.
3. **Within-notification bullet credit** (RG non-blocking cleanup).
   The "Coalescence covers both directions of adjacency" bullet in
   *What Changes* previously said the parser collapses within-
   notification same-kind entries. That's not what the rest of the
   proposal says: `parse_replay_entries_from_params` is unchanged and
   returns a `Vec<ReplayEntry>` per notification; the helper walks that
   vec and collapses. The bullet now matches the helper narrative.
   Normative spec + tasks were already correct; this is a wording-only
   fix applied before merge to keep the proposal internally consistent.
