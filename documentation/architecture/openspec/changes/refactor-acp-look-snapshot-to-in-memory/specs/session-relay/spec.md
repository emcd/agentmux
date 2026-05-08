## MODIFIED Requirements

### Requirement: ACP Look Snapshot Contract

Relay look SHALL support ACP-backed target sessions using an in-memory
snapshot held by the shared per-target ACP worker/client. Snapshot data
SHALL NOT be persisted to disk; the upstream ACP server is the
authoritative source of conversation history and provides full replay
via `session/load` on worker reconnect.

For ACP targets, relay SHALL:
- use the same shared per-target ACP worker/client used by ACP send
  lifecycle and prompt execution,
- ingest replay content from `session/load` as baseline snapshot
  replacement (in-memory),
- ingest replay content from live `session/update` as append in ACP
  receive order (in-memory),
- preserve source order (oldest -> newest) without dedupe in MVP,
- retain at most 1000 ACP snapshot entries per session in the in-memory
  buffer,
- evict oldest entries first when retention exceeds 1000,
- return look results ordered oldest -> newest,
- avoid spawning a second ACP client for steady-state look requests,
- read look snapshot via a non-draining accessor exposed by the ACP
  client (the existing draining `take_replay_entries` accessor is
  reserved for non-look consumers such as the debug TUI binary).

Canonical ACP snapshot entry vocabulary SHALL be:
- `kind = "user"` with `lines: string[]`
- `kind = "agent"` with `lines: string[]`
- `kind = "cognition"` with `lines: string[]`
- `kind = "invocation"` with `call_id: string` (upstream-issued
  correlation token), `status: "pending"|"completed"`,
  `invocation: object` (pass-through tool-call structure), and optional
  `result: object` (pass-through tool-result structure when status is
  completed). Coalesced form: a single entry carries both the call and
  its result; no separate result entry.
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

- **WHEN** relay observes a `tool_call` replay entry followed by a matching `tool_call_update`
- **THEN** the in-memory snapshot carries one entry with `kind="invocation"`, `status="completed"`, the original `invocation` payload, and the `result` payload inline
- **AND** no separate `kind="result"` entry is emitted

#### Scenario: Look returns fresh-but-empty during cold-start prime

- **WHEN** relay restarts and serves `look` before the worker has completed `session/load`
- **THEN** the response carries `freshness=stale` with `stale_reason_code=acp_worker_initializing`
- **AND** subsequent `look` after prime completes returns the full upstream-replayed transcript

#### Scenario: Evict oldest in-memory snapshot entries beyond retention cap

- **WHEN** the in-memory ACP snapshot reaches 1000 entries for a target session
- **AND** an additional entry is ingested
- **THEN** the oldest entry is evicted
- **AND** look responses continue to return up to 1000 most recent entries
