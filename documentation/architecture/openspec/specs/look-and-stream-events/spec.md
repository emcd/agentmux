# look-and-stream-events Specification

## Purpose

Look operation (transport-agnostic + per-transport), persistent client streams, Hello registration, recipient routability, and stream event contracts.

## Requirements

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required) — MAY be a bare session id (resolved within the
  requester's bound/dispatch bundle) or a peer-qualified `<session>@<bundle>`
  id that selects a peer bundle by suffix (consistent with `Send` target
  routing)
- `lines` (optional)
- `offset` (optional; default `0`) — for ACP targets, pages the entry
  window backward from the newest end; for tmux targets only `0` is valid
- `bundle_name` (optional, redundant) — treated as the dispatch-bundle echo; it
  SHALL NOT select or reject a peer bundle. The peer bundle is derived solely
  from the `target_session` suffix

The relay SHALL resolve the look target's hosting bundle from the
`target_session` suffix and SHALL capture the snapshot from that bundle's
runtime context.

Authorization posture for `look` SHALL be:

- default scope `self`
- self-inspection (requester equals target) is always permitted; this shortcut
  applies to same-bundle look only
- same-bundle inspection of a different session requires `home`
- cross-bundle inspection of a peer bundle's session requires `all`,
  evaluated against the requester's own (dispatch) bundle policy; `home`
  confers no authority beyond the requester's own bundle

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request target is bare and omits `bundle_name`
- **THEN** relay resolves the target within the associated (bound) bundle

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching the associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Resolve peer bundle from target suffix

- **WHEN** look request target is `<session>@<peer-bundle>` where
  `<peer-bundle>` differs from the requester's dispatch bundle
- **AND** `<peer-bundle>` is configured on the relay and `<session>` is a member
- **AND** requester policy has `look = "all"`
- **THEN** relay captures the snapshot from `<peer-bundle>`'s runtime and
  returns `bundle_name = <peer-bundle>` with the requester echoed in its own
  dispatch bundle

#### Scenario: Reject unknown peer bundle

- **WHEN** look request target names a bundle that is not configured on this
  relay
- **THEN** relay rejects request with `validation_unknown_bundle`

#### Scenario: Reject unknown peer session

- **WHEN** look request target names a configured peer bundle but a session that
  is not a member of that bundle
- **THEN** relay rejects request with `validation_unknown_target`

#### Scenario: Deny cross-bundle look under home scope

- **WHEN** requester targets a session in a peer bundle
- **AND** requester policy has `look = "home"` or narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Reject nonzero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is present and not equal to `0`
- **THEN** relay rejects request with `validation_offset_unsupported`

#### Scenario: Accept zero offset on tmux target

- **WHEN** look request targets a tmux session
- **AND** `offset` is omitted or equal to `0`
- **THEN** relay accepts request and proceeds with the look operation

### Requirement: Look Capture Window Bounds

Look capture window SHALL use deterministic bounds, with the default
keyed on target type:

- default `lines = 120` for tmux targets
- default `lines = 50` for ACP targets
- maximum `lines = 1000`
- valid range `1..=1000`

For ACP targets, the entry window SHALL be selected from the newest end of
the available buffer using `offset`:

- the window is the half-open range `[start, end)` over the full ordered
  entry buffer, where `end = entries_total.saturating_sub(offset)` and
  `start = end.saturating_sub(lines)` (two saturating subtractions, no other
  arithmetic)
- when `offset >= entries_total`, the window SHALL be empty
  (`returned_entries_count = 0`); this is a normal terminal page, NOT an error

#### Scenario: Apply default tmux line window

- **WHEN** look request for a tmux target omits `lines`
- **THEN** relay captures using default `lines = 120`

#### Scenario: Apply default ACP entry window

- **WHEN** look request for an ACP target omits `lines`
- **THEN** relay windows using default `lines = 50`

#### Scenario: Reject out-of-range line window

- **WHEN** look request includes `lines` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_lines`

#### Scenario: Window ACP entries backward from newest end

- **WHEN** ACP look request includes `offset` with `offset < entries_total`
- **THEN** relay returns the window ending `offset` entries back from the
  newest entry, sized by `lines`

#### Scenario: Offset beyond available entries yields empty window

- **WHEN** ACP look request includes `offset >= entries_total`
- **THEN** relay returns `snapshot_entries=[]` with `returned_entries_count=0`
- **AND** relay does NOT return an error
- **AND** `entries_total` still reflects the full buffer count

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_format` (`lines` | `structured_entries_v1`)

`bundle_name` is retired from look responses; bundle context is recoverable
from the `target_session` suffix.

When `snapshot_format = "lines"`, responses SHALL include:
- `snapshot_lines` (`string[]`)

When `snapshot_format = "structured_entries_v1"`, responses SHALL include:
- `snapshot_entries` (`object[]`) using the canonical structured entry
  vocabulary (transport-neutral; produced by ACP today).

For ACP targets, successful relay look responses SHALL additionally include:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `entries_total` (`number`, required) — count of all entries available in
  the buffer before windowing; reflects the full buffer on every response,
  including stale and empty snapshots
- `returned_entries_count` (`number`, required) — count of entries in
  `snapshot_entries` after windowing; SHALL equal the length of
  `snapshot_entries`
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when unavailable)

ACP stale reason vocabulary:

- `acp_worker_initializing`
- `acp_worker_unavailable`
- `acp_snapshot_prime_timeout`
- `acp_stream_stalled`

#### Scenario: Return canonical look payload for tmux target

- **WHEN** look succeeds for tmux target
- **THEN** relay returns `snapshot_format="lines"`
- **AND** includes ordered `snapshot_lines` from oldest to newest
- **AND** ACP additive fields are omitted

#### Scenario: Return structured-entries look payload for ACP target

- **WHEN** look succeeds for ACP target
- **THEN** relay returns `snapshot_format="structured_entries_v1"`
- **AND** includes `snapshot_entries`
- **AND** includes required ACP additive fields `freshness`,
  `snapshot_source`, `entries_total`, and `returned_entries_count`

#### Scenario: Report entry counts for windowed ACP look

- **WHEN** ACP look returns a window narrower than the full buffer
- **THEN** `entries_total` reflects the full buffer count
- **AND** `returned_entries_count` equals the length of `snapshot_entries`
- **AND** `returned_entries_count <= entries_total`

#### Scenario: Keep required ACP fields when snapshot is empty

- **WHEN** ACP look succeeds with `snapshot_entries=[]`
- **THEN** relay still includes required `freshness`, `snapshot_source`,
  `entries_total`, and `returned_entries_count`
- **AND** `returned_entries_count=0`

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
  client (the existing draining `take_replay_entries` accessor is
  reserved for non-look consumers such as the debug TUI binary).

Canonical ACP snapshot entry vocabulary SHALL be:

- `kind = "user"` with `lines: string[]` and a `source` discriminator
  (`PromptPath` for an operator\'s local submission, `ReaderThread`
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
  Pending entry\'s buffer position when `tool_call` is parsed and
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
  mutated in place to reflect the new state; the buffer\'s entry
  count does not advance
- **AND** the recorded `buffer_position` of the Pending entry
  remains valid under cap eviction: the parser tracks any
  position shift caused by oldest-first eviction of the buffer
  front, and mutates the entry at the post-eviction position;
  if the Pending Invocation itself was evicted, the parser
  falls through to the replay-baseline affordance (single
  Completed entry)

#### Scenario: Pending tool call evicted before completion

- **WHEN** relay observes a `tool_call` and the buffer\'s 1000-entry
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
  ONLY the buffer entry that recorded call A\'s original Pending
  notification; it does not touch the buffer entry for call B
- **AND** call B\'s final state is preserved at the buffer
  position that recorded its own Pending notification

#### Scenario: Out-of-order terminal tool_call_update mutates the right entry

- **WHEN** relay observes `tool_call(A)`, `tool_call(B)`,
  `tool_call_update(B)` (with terminal status + result),
  `tool_call_update(A)` (with terminal status + result)
- **THEN** the buffer entry that recorded call A\'s Pending
  notification is now `status="completed"` with call A\'s result
  payload
- **AND** the buffer entry that recorded call B\'s Pending
  notification is now `status="completed"` with call B\'s result
  payload
- **AND** the buffer holds exactly two `Invocation` entries, neither
  carrying the other call\'s result

#### Scenario: Look returns fresh-but-empty during cold-start prime

- **WHEN** relay restarts and serves `look` before the worker has completed `session/load`
- **THEN** the response carries `freshness=stale` with `stale_reason_code=acp_worker_initializing`
- **AND** subsequent `look` after prime completes returns the full upstream-replayed transcript

#### Scenario: Evict oldest in-memory snapshot entries beyond retention cap

- **WHEN** the in-memory ACP snapshot reaches 1000 entries for a target session
- **AND** an additional entry is ingested
- **THEN** the oldest entry is evicted
- **AND** look responses continue to return up to 1000 most recent entries

### Requirement: ACP Look Freshness Derivation

Relay SHALL evaluate ACP look freshness deterministically from shared worker
state and update recency.

Deterministic thresholds:

- `acp_look_prime_timeout_ms = 750`
- `acp_stream_stalled_after_ms = 5000`

Authoritative age source precedence SHALL be:
1. `last_acp_frame_observed_at_ms` (updated by background reader on each
   `session/update` notification)
2. `last_snapshot_update_ms`
3. age unavailable (omit `snapshot_age_ms`)

Freshness predicate order SHALL be:
1. `worker_state=unavailable` => `freshness=stale`,
   `stale_reason_code=acp_worker_unavailable`.
2. snapshot empty:
   - prime timeout => `stale_reason_code=acp_snapshot_prime_timeout`
   - otherwise => `stale_reason_code=acp_worker_initializing`
3. snapshot non-empty:
   - `worker_state=busy` => `freshness=fresh` (MUST NOT emit
     `acp_stream_stalled`)
   - `worker_state=initializing` => `freshness=stale`,
     `stale_reason_code=acp_worker_initializing`
   - `worker_state=available` => `freshness=stale` with
     `stale_reason_code=acp_stream_stalled` only when stalled threshold is
     exceeded using authoritative age source precedence.

Freshness vocabulary semantics under continuous-reader model:

- `LiveBuffer` means "background reader thread is alive and feeding updates"
  (previously: "served from in-memory after request-scoped drain")
- `acp_stream_stalled` means "reader alive but observed N seconds of silence
  on the ACP stream" (previously: "drain window saw nothing")

Wire tokens (`Fresh`/`Stale`, `LiveBuffer`/`None`) are unchanged.

Relay SHALL treat machine freshness status as response-visible state for ACP
look.

Inscriptions/events MAY include the same freshness data as additive telemetry
but SHALL NOT be the sole machine carrier.

#### Scenario: Mark ACP look stale when worker is unavailable

- **WHEN** ACP look is served and shared worker state is unavailable
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_worker_unavailable`

#### Scenario: Keep ACP look fresh while worker is busy and snapshot exists

- **WHEN** ACP look is served with non-empty snapshot entries
- **AND** shared worker state is busy
- **THEN** relay returns `freshness=fresh`
- **AND** relay does not emit `stale_reason_code=acp_stream_stalled`

#### Scenario: Mark ACP look stale when available worker stream is stalled

- **WHEN** shared ACP worker is available
- **AND** no new ACP updates are observed for at least 5000ms using
  authoritative age-source precedence
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_stream_stalled`

#### Scenario: Background reader updates last_acp_frame_observed_at on each update

- **WHEN** background reader receives a `session/update` notification
- **THEN** `last_acp_frame_observed_at_ms` is updated to the current time
- **AND** freshness stalled threshold uses this timestamp as authoritative age

### Requirement: Persistent Relay Client Streams

Relay SHALL support long-lived full-duplex Unix socket client streams.

Client request/response frames and relay-pushed event frames SHALL share the
same stream connection.

Relay SHALL reject protocol frames received before successful `hello`
registration with `validation_missing_hello`.

#### Scenario: Accept request/response exchange on persistent stream

- **WHEN** a client opens relay stream and completes `hello`
- **THEN** client can send request frames on that stream
- **AND** relay returns response frames on that same stream without closing it

#### Scenario: Reject request before hello

- **WHEN** client sends request frame before successful `hello`
- **THEN** relay rejects frame with `validation_missing_hello`

### Requirement: Hello Registration Contract

Each client stream SHALL begin with a `hello` registration frame containing:

- `principal_id` (in `<id>@<namespace>` form)
- `identity_token`
- `schema_version`

`hello` SHALL carry identity and credential only. No transport class, mode, or
privilege field is accepted; relay SHALL reject unrecognized fields.

The relay process hosts a single Unix socket at `<state_root>/relay.sock` and
serves all configured bundles through that socket. The namespace portion of
`principal_id` SHALL serve as the connection type indicator:

- Session namespace (`@<bundle_name>`): relay SHALL look the bundle up in the
  bundle catalog and bind the connection to that bundle's runtime for the
  lifetime of the stream. If the bundle is not configured, relay SHALL reject
  with `validation_unknown_bundle`.
- Relay-wide namespaces (`@GLOBAL`, `@EXTERNAL`, `@RELAY`): relay SHALL NOT
  bind the connection to any bundle; the connection is relay-wide.

Credential verification and principal store lookup SHALL proceed as specified
by `add-identity-federation`.

If a second stream attempts `hello` for the same `principal_id` while the
current owner is live, relay SHALL reject the second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for session principal

- **WHEN** a client sends valid `hello` with `principal_id = "master@agentmux"`
- **AND** namespace `agentmux` maps to a configured bundle
- **AND** `master` maps to a configured bundle member
- **THEN** relay accepts hello and binds stream to bundle `agentmux`

#### Scenario: Accept hello for global user principal

- **WHEN** a client sends valid `hello` with `principal_id = "operator@GLOBAL"`
- **AND** credential is valid
- **THEN** relay accepts hello and registers connection relay-wide (no bundle binding)

#### Scenario: Reject hello for unknown bundle

- **WHEN** a client sends `hello` with `principal_id = "session@unknownbundle"`
- **AND** `unknownbundle` is not configured on the running relay
- **THEN** relay rejects with `validation_unknown_bundle`
- **AND** closes the connection without registering a stream

### Requirement: Static Recipient Routability

Static configured recipients from bundle session definitions SHALL remain
routable independent of active stream presence or prior `hello` from those
recipients.

#### Scenario: Route to configured recipient before recipient stream registration

- **WHEN** sender targets a configured bundle recipient
- **AND** recipient has no active stream registration
- **THEN** relay processes routing using configured recipient identity semantics
- **AND** does not reject solely for missing recipient `hello`

### Requirement: Relay Stream Event Contract

Relay pushed event frames SHALL include:

- `event_type`
- `target_session`
- `created_at`

`target_session` SHALL carry the canonical `session@bundle` form per the
Canonical Session Identity requirement. `bundle_name` is retired; bundle
context is recoverable from the `target_session` suffix.

Event types SHALL include:

- `incoming_message`
- `delivery_outcome`

`incoming_message` payload SHALL include:

- `message_id`
- `sender_session`
- `body`
- optional `cc_sessions`

`sender_session` and `cc_sessions` SHALL carry the bare canonical
`session@namespace` form obtained via the non-decorating identity accessor,
except that a `sender_session` attributed to a cross-relay origin SHALL carry
the `<origin>!<peer-name>` form specified by the `cross-relay-routing`
capability. That origin segment is copied from the asserting peer rather than
validated, so it carries whatever was stamped and SHALL be emitted unaltered
whether or not it names a routable recipient. Either form SHALL be emitted
without
decoration: these fields SHALL NOT carry the decorating pane-header form
(`Display Name <session:session_name>`) produced by `render_address`. The
pane-envelope From/To/Cc header is the only surface that uses the decorating
form; the `incoming_message` machine event fields are exempt from it.

`delivery_outcome` payload SHALL include:

- `message_id`
- `phase` (`routed`|`delivered`|`failed`|`not_submitted`|`submission_unknown`)
- `outcome` (`success`|`failed`|`not_submitted`|`submission_unknown`|null)
- optional `reason_code`
- optional `reason`

`delivery_outcome` SHALL be the canonical machine completion/update carrier for
stream-path delivery updates and SHALL be keyed by `message_id`.

`phase=routed` SHALL be diagnostic metadata and SHALL set `outcome=null`.

Terminal updates SHALL keep existing external vocabulary:

- delivered terminal: `phase=delivered`, `outcome=success`
- failure terminal: `phase=failed`, `outcome=failed`
- provable non-delivery terminal: `phase=not_submitted`,
  `outcome=not_submitted`
- indeterminate-submission terminal: `phase=submission_unknown`,
  `outcome=submission_unknown`

`not_submitted` and `submission_unknown` SHALL each carry their own `phase` and
`outcome` spelling rather than being reported as a failure terminal. They make
opposite evidentiary claims — one asserts that no target-side effect occurred,
the other that such an effect cannot be excluded — and collapsing either into
`failed` would assert a non-delivery the relay cannot support.

Relay terminal state `dropped_on_shutdown` SHALL map to:

- `phase=failed`
- `outcome=failed`
- `reason_code=dropped_on_shutdown`
- propagated `reason` text when available

#### Scenario: Push incoming message event to ui stream

- **WHEN** relay delivers message to connected ui recipient
- **THEN** relay pushes `incoming_message` event frame on that stream

#### Scenario: Emit bare canonical sender and cc identity in incoming_message

- **WHEN** the sender identity is `session_name = "alice@bundle"` with
  `display_name = "Alice Cooper"` and a co-recipient has the same shape
- **THEN** the `incoming_message` event `sender_session` equals `"alice@bundle"`
- **AND** each entry in `cc_sessions` is the bare canonical `session@namespace`
  id
- **AND** neither field carries the decorating
  `Display Name <session:session_name>` form

#### Scenario: Emit a cross-relay sender undecorated

- **WHEN** the delivered sender is attributed to a cross-relay origin
  `coordinator@agentmux` asserted by a peer this relay names `bravo`
- **THEN** the `incoming_message` event `sender_session` equals
  `"coordinator@agentmux!bravo"`
- **AND** the field does not carry the decorating
  `Display Name <session:session_name>` form

#### Scenario: Emit a relay-wide cross-relay origin

- **WHEN** the delivered sender is attributed to a cross-relay origin
  `operator@GLOBAL` asserted by a peer this relay names `bravo`
- **THEN** the `incoming_message` event `sender_session` equals
  `"operator@GLOBAL!bravo"`

#### Scenario: Push routed diagnostic update

- **WHEN** relay resolves stream routing for a target delivery
- **THEN** relay pushes `delivery_outcome` with `phase=routed`
- **AND** sets `outcome=null`

#### Scenario: Push terminal delivery outcome update

- **WHEN** relay records terminal delivery outcome for message target
- **THEN** relay pushes `delivery_outcome` event frame
- **AND** includes canonical `phase` and `outcome` values

#### Scenario: Emit an evidence-bearing terminal outcome under its own spelling

- **WHEN** relay records a terminal delivery outcome of `not_submitted` or
  `submission_unknown` for a message target
- **THEN** `delivery_outcome` carries that spelling as both `phase` and
  `outcome`
- **AND** does not report it as `phase=failed`

#### Scenario: Map dropped_on_shutdown to failed terminal update

- **WHEN** relay terminal state for a target is `dropped_on_shutdown`
- **THEN** `delivery_outcome` includes `phase=failed`
- **AND** includes `outcome=failed`
- **AND** includes `reason_code=dropped_on_shutdown`

#### Scenario: Emit canonical target identity in delivery event

- **WHEN** relay delivers a message to session `"relay"` in bundle `"agentmux"`
- **THEN** delivery event includes `target_session = "relay@agentmux"`

### Requirement: Stream Failure Semantics

Relay SHALL fail fast on malformed hello/protocol frames.

Relay SHALL surface stream disconnect events through runtime diagnostics and
continue serving other active streams.

#### Scenario: Reject malformed hello payload

- **WHEN** client sends malformed or invalid hello frame
- **THEN** relay rejects with structured validation error
- **AND** does not register stream identity

#### Scenario: Continue serving other streams after one disconnect

- **WHEN** one client stream disconnects unexpectedly
- **THEN** relay records diagnostic event
- **AND** continues serving other active client streams
