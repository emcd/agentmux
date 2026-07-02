# Design: Aggregate ACP streaming chunks into complete turns

## Context

The agentmux relay relays look snapshots for ACP-backed target sessions by
reading the in-memory `Vec<ReplayEntry>` held by the shared per-target
`AcpStdioClient`. The buffer is appended to from two sources:

- `session/load` baseline replacement (one-shot on worker reconnect),
- live `session/update` ingestion (one notification per call from the
  upstream ACP server, often multiple per turn during streaming).

Today, the live ingestion path maps each chunk notification to one entry via
`parse_replay_entries_from_params` in `src/acp/client.rs`, then funnels the
result straight into `append_replay_entries`, which is currently a
`buffer.extend(entries)` followed by a ring-bound drain. There is no
within-buffer coalescence step. A multi-chunk assistant turn therefore
produces N fragment entries, not one coherent entry.

The look snapshot contract (`session-relay` spec, **ACP Look Snapshot
Contract** requirement) fixes the per-entry vocabulary (`lines: string[]` on
`User`/`Agent`/`Cognition`/`Update`) but says nothing about how many entries a
turn produces — that is a mechanical decision of the buffer-write path.

## Goals / Non-Goals

- Goals:
  - Coalesce adjacent same-kind ACP replay entries (User, Agent, Cognition,
    Update with matching `update_kind`) into a single entry whose `lines`
    array holds the concatenated content of the source entries in receive
    order.
  - Preserve all line content; no content is dropped or reordered.
  - Preserve the existing 1000-entry cap; coalescence happens before cap
    enforcement so a long-turn conversation does not blow past the cap
    prematurely.
  - Keep the change local to ACP transport (`src/acp/`); zero consumer-side
    code changes.
  - End-to-end streaming is NOT a goal. The relay, MCP, CLI, and TUI all
    continue to use the existing request/response `look` model. The
    transport writes a buffer that already represents coherent turns; the
    consumers poll that buffer at their existing cadence.
- Non-Goals:
  - Replacing the per-tool-call `tool_call` + `tool_call_update` merge
    mechanism (already implemented; covers `Invocation` only).
  - Dedupe of distinct same-kind lines (e.g., an upstream retry delivering
    the same chunk twice). The current contract explicitly disallows dedupe,
    and coalescence is orthogonal: same-kind adjacency is necessary but not
    sufficient for dedupe (same-kind chunks are common and expected in a
    single assistant turn).
  - Tmux-side coalescence. Tmux does not stream the way ACP does;
    `TmuxTransport` writes complete panes, not chunks. No equivalent
    quality bug exists.
  - UI transport coalescence. The UI transport is request/response (`set
    text/get text`) and does not produce replay entries.
  - Adding a per-coder configuration knob. Coalescence is unconditional; the
    only knob is the buffer cap (1000), which is unchanged.

## Decisions

### Decision: Write-path coalescence, in `append_replay_entries`

The coalescence step lives at the buffer-append boundary, not at the parser
boundary. Rationale:

- The parser may produce one or many entries per notification (a single
  `session/update` payload can carry a JSON array). Both single- and
  multi-entry cases need coalescence against the buffer tail (when the new
  batch starts with the same kind as the existing tail) and within the batch
  (when consecutive entries in the batch share a kind).
- Moving the step into `append_replay_entries` means the parser stays a pure
  mapping from `session/update` wire payload to `Vec<ReplayEntry>` with no
  duplicate concerns, and tests of the parser don't have to know about the
  buffer state.
- The alternative (a parser-side pass that knows the current buffer tail)
  would require either threading the buffer into the parser (changing its
  signature) or doing a re-walk of the just-parsed entries against the tail
  (a second pass with the same logic). One helper at the boundary is
  simpler.
- The `mpsc::sync_channel` write path in
  `AcpStdioClient::dispatch_session_update` is one call site of
  `append_replay_entries` today. The prompt-path call from
  `AcpStdioClient::prompt` (which appends the operator's submitted prompt
  to the buffer before the agent response arrives) is the other. The
  amendment splits this into two helpers — one coalescing (used by the
  reader-thread path) and one non-coalescing (used by the prompt path) —
  so the rule's scope (read-path only) is enforced at the call boundary
  rather than at every call site. See the "Outgoing prompt User entries
  do not coalesce" decision below.

Alternatives considered:

- **Read-path coalescence in `replay_entries_to_snapshot_entries`**:
  walks `Vec<ReplayEntry>` -> `Vec<StructuredEntry>`. Pros: zero change to
  the buffer shape, consumers stay unchanged, the snapshot already has
  the desired coalesced form before any consumer sees it. Cons: returns a
  snapshot that disagrees with the buffer; the buffer is still fragmented
  for any future direct buffer reader. The wider problem (the buffer is the
  authoritative in-memory transcript for replay baselines and for the TUI's
  cursor accessor) is not solved. Rejected on blast-radius grounds: the
  snapshot path is read-only; the buffer path is the single source of truth.
- **Read-path coalescence in `derive_acp_look_snapshot`**: same issue,
  deeper in the stack. Rejected for the same reason.
- **Per-update coalescence in the parser**: handles within-notification
  collapse but not tail extension across notifications. Requires a second
  pass anyway. Rejected as adding complexity without resolving the
  cross-notification case.

### Decision: Coalescence scope is `User`, `Agent`, `Cognition`, `Update (matching update_kind)`; NOT `Invocation`

`Invocation` entries are per-call, keyed by upstream-issued `call_id`. The
existing `tool_call` + `tool_call_update` merge in the parser already
collapses the typical pair; the only way two `Invocation` entries can be
adjacent is if they are two distinct tool calls. Merging them would lose the
per-call boundary the consumers rely on.

`Update` entries carry a discriminator (`update_kind`) that may differ between
adjacent entries (e.g., a "permission_requested" update followed by a
"plan_execution" update); the two should not merge even if the buffer
position is adjacent. Two `Update` entries with matching `update_kind` (e.g.,
a multi-line unknown kind that arrives as two separate notifications) merge.

The simpler "merge any adjacent same-variant" rule is wrong: it would either
under-merge `Update` (losing `update_kind` semantics) or over-merge
`Invocation` (losing per-call boundaries). The discriminator-aware rule is
correct and matches the field set on `ReplayEntry`.

### Decision: Within-notification collapse at the helper boundary, not in the parser

`parse_replay_entries_from_params` returns `Vec<ReplayEntry>` for a single
notification. Some notifications carry a JSON array and produce multiple
entries. The coalescence rule walks both (1) the buffer tail vs the new
batch's first entry, and (2) consecutive pairs within the new batch. Because
the batch is one `Vec<ReplayEntry>` arriving at `append_replay_entries`, the
helper can walk it once and produce the merged result without the parser
having to know about adjacency.

This means: a single notification that contains three same-kind entries
arrives as `[E1, E2, E3]`. The helper walks and produces `[merged(E1, E2,
E3)]` regardless of what is at the buffer tail. If the buffer tail is
already the same kind, the helper extends the tail's `lines` with E1's
`lines`, then absorbs E2 and E3 the same way. The two cases (single-vs-multi
entry per notification, with-vs-without a tail collision) collapse to one
helper that always walks the entries.

### Decision: Outgoing prompt User entries do not coalesce (Option A)

`AcpStdioClient::prompt` appends the operator's submitted prompt to the
replay buffer immediately so `look` reflects the submission before the
agent response arrives. The same `append_replay_entries` function is on
that path. If the helper blindly coalesced adjacent same-kind entries, two
prompts submitted in rapid succession (no agent response in between) would
be merged into one `User` entry whose `lines` array contains both messages
textually concatenated.

The amendment rules that out: coalescence applies only to ingestion from the
ACP reader thread (`session/update` notifications and `session/load`
replay history); the prompt path uses a non-coalescing append that retains
today's behavior of pushing the new `User` entry directly.

Rationale:

- Each prompt is a distinct operator submission with its own intent.
  Conflating two prompts would erase the per-submission boundary operators
  rely on for cancel/follow-up semantics.
- Chat-UI convention treats each send as one bubble; merging two sends
  breaks the visual model operators expect.
- Conflation would conflate two distinct agent round-trips. If a prompt
  is submitted while the agent is still processing the previous one, the
  `User` entries represent two separate agent interactions; merging them
  into one entry misrepresents the timeline.

Alternatives considered:

- **Option B (prompt User entries DO coalesce)**: rule is uniform across
  paths. The downside is that two distinct agent round-trips collapse into
  one entry, and the operator's "two sends, two agent responses" mental
  model is lost. RG review offered this as an explicit option but called
  out that the existing `Non-Draining Replay Buffer Accessor` scenario (a
  user prompt appends a `ReplayEntry::User`) would need to be loosened to
  "append or extend the tail User entry". Loose. Option A preferred
  because it preserves the existing accessor contract verbatim while
  scoping coalescence to the reader-thread ingestion path where the
  benefit (turn coherence) clearly outweighs the cost.

Implementation: the amendment provides two helpers in `src/acp/client.rs`,
both `pub(in crate::acp)`:

- `coalesce_replay_entries_on_append(buffer, new_entries)` — coalescing,
  called only from `dispatch_session_update`.
- `append_prompt_user_entry(buffer, user_entry)` — non-coalescing,
  called only from `prompt`. Pushes the entry, enforces the cap.

Callers that ever want to add a third path (e.g., a future manual `inject`
for diagnostics) MUST pick one or the other explicitly; the helpers'
existence at the boundary prevents accidental coalescence-by-default.

### Decision: Tests live in `tests/unit/acp/replay_coalescence.rs` plus an integration scenario

Unit tests cover the helpers directly: same-kind vs different-kind,
single-entry batch, multi-entry batch, tail collision vs no-tail-collision,
`Invocation` non-merging, `Update` discriminant-aware merging, empty-line
filtering (the parser already filters empty `lines: Vec<String>`; the helper
sees those empty-line entries dropped before it, but if any arrive it should
not panic on empty `lines`), the prompt-path non-coalescing append
(asserts that two back-to-back prompt submissions remain two `User`
entries when the buffer tail is `User`), and a session/load replay-history
shaped input vec (multi-entry same-kind per turn with mixed kinds across
turns, mimicking the `session/load` replay the reader thread ingests on
reconnect). Integration tests cover one streaming end-to-end scenario:
spawn an ACP session that streams a multi-chunk assistant turn; observe
that `look` returns one entry per turn kind, not N fragment entries.

### Risks / Trade-offs

- **Risk**: coalescence reduces the entry count of chatty sessions, which
  extends the buffer's effective temporal reach. With the 1000-entry cap
  preserved, a single chatty turn now occupies one entry instead of N.
  Mitigation: this is the desired effect (coherent blocks, not fragments).
  No new cap knob in v1.
- **Risk**: future readers of the buffer (e.g., a future replay cursor that
  expects one cursor position per entry) will see fewer entries. Mitigation:
  the `ReplayCursor` accessor in `acp-client` spec returns "entries since
  last read position" — coalescence reduces the number of positions but
  preserves content. The cursor's correctness rests on ordering, which is
  preserved.
- **Risk**: an upstream regression that re-delivers the same chunk
  repeatedly now silently coalesces into one entry with N copies of the
  same line. Mitigation: a future coalescence-protected unit test asserts
  that two identical-line adjacent chunks produce one entry with two lines
  (the helper extends `lines`, not dedupes). The dedupe story (if ever
  needed) is a separate concern from coalescence.
- **Trade-off**: read-path coalescence would have been a smaller-diff change
  (no buffer-append refactor). It was rejected on blast-radius grounds —
  the buffer is the authoritative source of truth and read-path coalescence
  leaves it fragmented. The write-path refactor is small (one helper in
  `src/acp/client.rs`) and matches the precedence set by the `tool_call` +
  `tool_call_update` merge (which also lives at the parser boundary).

## Migration Plan

- No migration. The change is wire-format-compatible, configuration-free,
  and consumer-free. Existing test fixtures that assert one-entry-per-chunk
  pass vacuously (one entry is its own coalescence result). Existing
  scenarios asserting buffer cap and ordering hold verbatim under the new
  helper.
- Rollback path: revert the helper change; tests fail at the
  `tests/unit/acp/replay_coalescence.rs` boundary. No consumer rollback
  needed.

## Open Questions

- (none at draft time; flag at any review checkpoint if a reviewer finds
  one)
