## MODIFIED Requirements

### Requirement: ACP Look Snapshot Contract

For ACP targets, relay SHALL:

- use the same shared per-target ACP worker/client used by ACP send
  lifecycle and prompt execution,
- ingest replay content from `session/load` as baseline snapshot
  replacement (in-memory),
- ingest replay content from live `session/update` as append in ACP
  receive order (in-memory),
- preserve source order (oldest -> newest) without dedupe,
- retain at most 1000 ACP snapshot entries per session in the in-memory
  buffer,
- evict oldest entries first when retention exceeds 1000,
- return look results ordered oldest -> newest,
- avoid spawning a second ACP client for steady-state look requests,
- read look snapshot via a non-draining accessor exposed by the ACP
  client (see the `acp-client` capability's `Non-Draining Replay Buffer
  Accessor` requirement, which defines the snapshot and cursor accessors
  every consumer uses).

Canonical ACP snapshot entry vocabulary SHALL be:

- `kind = "user"` with `lines: string[]` and a `source` discriminator
  (`PromptPath` for an operator's local submission, `ReaderThread`
  for chunks parsed from `session/update` / `session/load`).
  Cross-source User adjacency is not coalesced; same-source
  adjacency coalesces under the dedicated User-rule (the
  reader-thread coalescence helper enforces this; the snapshot
  boundary drops the source field).
- `kind = "agent"` with `lines: string[]`
- `kind = "cognition"` with `lines: string[]`
- `kind = "invocation"` with `call_id: string` (upstream-issued
  correlation token), `status: "pending"|"completed"`,
  `invocation: object` (pass-through tool-call structure), and optional
  `result: object` (pass-through tool-result structure when status
  is completed). Coalesced form: a single entry carries BOTH the
  call and its complete lifecycle through the terminal status; no
  separate `result` entry is emitted. The coalescence is per-
  `call_id` and operates by in-place mutation of the buffer entry
  tracked by the parser-side accumulator (the parser records the
  Pending entry's buffer position when `tool_call` is parsed and
  mutates that position in place when the matching `tool_call_update`
  with `status="completed"` arrives), NOT by buffer-position
  adjacency. v1 scope: `Pending` -> `Completed` transition only;
  the v2 statuses `failed` / `in_progress` and the broader v2
  patch-fields surface are deferred to a separate OpenSpec
  change.
- `kind = "update"` with `update_kind: string`, `lines: string[]` for
  fallback unknown/unsupported updates (MUST NOT be dropped).

Relay SHALL NOT inject ANSI/control sequences into ACP snapshot
entries.

Relay restart behavior SHALL be:

- on relay startup, the worker reconnects to the upstream session via
  `session/load` using the persisted `acp_session_id` (see ACP Session
  Identity Persistence Ownership),
- if no usable persisted session id exists, the worker creates a new
  upstream session via `session/new`,
- the in-memory snapshot is rebuilt from the upstream replay; first
  `look` during prime returns fresh-but-empty with the appropriate
  `stale_reason_code` (see ACP Look Freshness Derivation).

#### Scenario: Use shared ACP worker as authoritative look snapshot writer

- **WHEN** relay serves ACP send and ACP look for one target session
- **THEN** both operations use one shared per-target ACP worker/client
- **AND** relay does not create a separate look-only ACP client for that target

#### Scenario: Replace baseline from load then append live updates in order

- **WHEN** relay receives `session/load` replay for target session
- **AND** later observes live `session/update` replay entries
- **THEN** relay replaces the in-memory snapshot baseline from load replay
- **AND** appends live snapshot entries in ACP receive order
- **AND** preserves oldest->newest ordering in look responses

#### Scenario: Preserve unknown replay kinds via fallback entry

- **WHEN** relay observes an unknown or unsupported replay/update kind
- **THEN** relay emits fallback entry `kind="update"`
- **AND** relay does not silently drop the observed update

#### Scenario: Coalesce tool call and result onto one invocation entry

- **WHEN** relay observes a `tool_call` replay entry followed by a
  matching `tool_call_update` (the matching is keyed by the
  upstream-supplied `call_id`, NOT by buffer-position adjacency)
- **THEN** the in-memory snapshot carries one entry with
  `kind="invocation"`, `status="completed"`, the original
  `invocation` payload, and the `result` payload inline
- **AND** no separate `kind="result"` entry is emitted
- **AND** if the matching `tool_call_update` arrives later than
  intervening notifications (other agent text, cognition, or
  other in-flight tool calls), the existing Pending entry is
  mutated in place to reflect the new state; the buffer's entry
  count does not advance
- **AND** the recorded `buffer_position` of the Pending entry
  remains valid under cap eviction: the parser tracks any
  position shift caused by oldest-first eviction of the buffer
  front, and mutates the entry at the post-eviction position;
  if the Pending Invocation itself was evicted, the parser
  falls through to the replay-baseline affordance (single
  Completed entry)

#### Scenario: Pending tool call evicted before completion

- **WHEN** relay observes a `tool_call` and the buffer's 1000-entry
  cap drains the front enough times that the Pending Invocation
  itself is evicted before the matching `tool_call_update` arrives
- **THEN** the parser removes the call_id from `pending_calls` at
  eviction time
- **AND** when the matching `tool_call_update` subsequently
  arrives, the parser falls through to the replay-baseline
  affordance (single Completed entry)
- **AND** no `kind="result"` entry is emitted and no out-of-bounds
  buffer access is attempted on a stale position

#### Scenario: Concurrent in-flight tool calls each produce one entry per call_id

- **WHEN** multiple tool calls are in flight concurrently and each
  emits its own `tool_call` followed by its matching `tool_call_update`
  (possibly out of arrival order)
- **THEN** the in-memory snapshot carries one entry per `call_id`
- **AND** an in-flight `tool_call_update` for call_id A mutates
  ONLY the buffer entry that recorded call A's original Pending
  notification; it does not touch the buffer entry for call B
- **AND** call B's final state is preserved at the buffer
  position that recorded its own Pending notification

#### Scenario: Out-of-order terminal tool_call_update mutates the right entry

- **WHEN** relay observes `tool_call(A)`, `tool_call(B)`,
  `tool_call_update(B)` (with terminal status + result),
  `tool_call_update(A)` (with terminal status + result)
- **THEN** the buffer entry that recorded call A's Pending
  notification is now `status="completed"` with call A's result
  payload
- **AND** the buffer entry that recorded call B's Pending
  notification is now `status="completed"` with call B's result
  payload
- **AND** the buffer holds exactly two `Invocation` entries, neither
  carrying the other call's result

#### Scenario: Look returns fresh-but-empty during cold-start prime

- **WHEN** relay restarts and serves `look` before the worker has completed `session/load`
- **THEN** the response carries `freshness=stale` with `stale_reason_code=acp_worker_initializing`
- **AND** subsequent `look` after prime completes returns the full upstream-replayed transcript

#### Scenario: Evict oldest in-memory snapshot entries beyond retention cap

- **WHEN** the in-memory ACP snapshot reaches 1000 entries for a target session
- **AND** an additional entry is ingested
- **THEN** the oldest entry is evicted
- **AND** look responses continue to return up to 1000 most recent entries
