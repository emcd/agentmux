## ADDED Requirements

### Requirement: ACP Replay Stream Adjacency Coalescence

The shared ACP client module SHALL coalesce adjacent same-kind replay entries
on ingestion from the ACP reader thread into the live replay buffer.
Coalescence SHALL preserve receive order and SHALL preserve all line content.

Coalescence applies to ingestion from the ACP reader thread only. The
reader-thread ingestion path serves two wire sources, both of which are in
scope for coalescence:
- `session/update` notifications (live streaming during a session).
- `session/load` replay history (worker reconnect / resume on relay
  restart). Coalescence covers both because both wire sources share the
  same reader-thread -> `dispatch_session_update` -> ingestion path.

Coalescence does NOT apply to the prompt path. The
`AcpStdioClient::prompt` submission flow appends the operator's submitted
prompt to the buffer immediately so `look` reflects the submission before
the agent response arrives; this flow uses a non-coalescing append helper
that retains a one-prompt-one-`User`-entry invariant regardless of the
buffer tail.

Coalescence applies to the following kinds:
- `User` — two adjacent `User` entries merge into one `User` entry whose
  `lines` array is the receive-order concatenation of the source entries'
  `lines` arrays. Source for `User` entries is `session/update`/`session/load`
  only; prompt-path `User` entries are never the target of coalescence.
- `Agent` — two adjacent `Agent` entries merge into one `Agent` entry by the
  same rule.
- `Cognition` — two adjacent `Cognition` entries merge into one `Cognition`
  entry by the same rule.
- `Update` — two adjacent `Update` entries merge only when their
  `update_kind` is identical; the merge is by the same rule and preserves
  the shared `update_kind`.

Coalescence does NOT apply to:
- `Invocation` — each `Invocation` entry represents one upstream-issued tool
  call (with optional coalesced result) and MUST NOT merge with an adjacent
  `Invocation` regardless of identity.
- any pair where the two adjacent entries are of different `kind`.
- prompt-path `User` entries (see the prompt-path boundary scenario below).

The buffer cap (1000 entries, per Replay Buffer Cap and Eviction) SHALL be
enforced AFTER coalescence; a chatty turn consuming a coalesced single
entry does not advance the cap faster than today.

#### Scenario: Within-batch same-kind entries collapse

- **WHEN** a single `session/update` ingestion carries two or more adjacent
  same-kind entries of `User`, `Agent`, or `Cognition`
- **THEN** the buffer holds one entry of that kind
- **AND** the entry's `lines` array concatenates the source entries' `lines`
  in receive order
- **AND** no line is dropped or reordered

#### Scenario: Tail-of-buffer same-kind entries extend in place

- **WHEN** the last entry of the buffer and the first entry of a new
  ingestion batch are the same kind (`User`, `Agent`, or `Cognition`)
- **THEN** the buffer's last entry is preserved as a single entry
- **AND** its `lines` array extends with the new entry's `lines` in receive
  order

#### Scenario: Different-kind adjacency does not merge

- **WHEN** two adjacent entries differ in `kind`
- **THEN** each entry is preserved as its own buffer position
- **AND** no entry crosses the kind boundary

#### Scenario: Invocation entries never merge across adjacency

- **WHEN** two adjacent entries are both `Invocation`
- **THEN** each entry is preserved as its own buffer position
- **AND** the per-call boundary is preserved

#### Scenario: Update merging is update_kind-aware

- **WHEN** two adjacent `Update` entries carry the same `update_kind`
- **THEN** they merge into one `Update` entry preserving the shared
  `update_kind`
- **WHEN** two adjacent `Update` entries carry different `update_kind`
- **THEN** each entry is preserved as its own buffer position

#### Scenario: Cap is enforced after coalescence

- **WHEN** coalescence reduces the would-be entry count such that the
  buffer would exceed the 1000-entry cap
- **THEN** the oldest entries are evicted first (per Replay Buffer Cap and
  Eviction)
- **AND** the buffer holds exactly the cap's most recent entries
- **AND** the most recent entry is the newly ingested one

#### Scenario: Prompt-path User appends preserve their entry boundary

- **WHEN** `AcpStdioClient::prompt` submits an operator prompt and appends
  the resulting `ReplayEntry::User` to the buffer
- **AND** the buffer's last entry is also a `User` entry (no `Agent` or
  `Cognition` response between them; e.g., two rapid back-to-back
  submissions)
- **THEN** a new `User` entry is pushed to the buffer
- **AND** the existing `User` tail entry is NOT extended
- **AND** the buffer count advances by one entry per prompt

#### Scenario: session/load replay-history ingestion coalesces like live streaming

- **WHEN** the ACP worker reconnects and ingests `session/load` replay
  history through the reader-thread ingestion path
- **AND** the replay history contains multiple adjacent same-kind entries
  per turn (e.g., several `agent_message_chunk` events for one assistant
  turn, or several `agent_thought_chunk` events for one thought block)
- **THEN** each per-turn same-kind run is coalesced into one buffer entry
  by the same rule as live `session/update` ingestion
- **AND** the buffer after reconnect holds one entry per turn kind, not
  N fragment entries
- **AND** no line content is dropped or reordered
