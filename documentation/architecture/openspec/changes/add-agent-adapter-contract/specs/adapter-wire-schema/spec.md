## ADDED Requirements

### Requirement: Newline-delimited JSON-RPC 2.0 framing with contract method mapping

The wire SHALL be newline-delimited UTF-8 JSON over stdio, one JSON-RPC 2.0 object per frame, each frame bounded by max_frame_bytes — this mapping reconciles D1: JSON-RPC version/method/result/error are used as specified here, not replaced. Requests carry `id` (string, opaque, pairing call and reply); unsolicited traffic uses notifications (no `id`). Agent ACP u64 ids travel in params (e.g. `acp_request_id`), never as the wire id. Methods (exact): `adapter.handshakeHello`, `adapter.requestChoice`, `relay.look`, `adapter.submit`, `adapter.persistState`, `adapter.lifecycleEvent` (notification), `relay.supervisionSignal` (notification, except `relay.ping` which is a request expecting `pong`). Choice/submit/look/persist requests stay open until their terminal outcome — exactly one terminal per correlation, delivered as the JSON-RPC response; post-resolution evidence arrives as diagnostic `adapter.lifecycleEvent`s, never a second disposition. JSON-RPC errors use code -32000 for all contract failures with `data: {code: <namespaced string>, reason: <string>, details?: <object>, correlation_id?: <string>}` — data.correlation_id present IFF the failed request carried one; for -32000 contract errors this is refusal-grade (absent → typed framing failure), EXCEPT correlation_invalid errors, which reject the correlation they would echo and are admitted by table match instead. Transport/parse errors (standard codes) carry no data-correlation requirement by construction. Contract errors SHOULD echo correlation_id in data when the failed request carried one; admission is ALWAYS by pending-table (binding + wire id) match, applied AFTER shape validity: a contract error missing a required echo is refused before admission. Table-only admission without correlation applies to transport-level errors, which name no correlation by construction. Results and errors carry no binding triple on the frame: they are interpreted under captured connection state (binding established at handshake plus generation currency); the triple lives on request/notification params only. protocol_major/minor on params MUST equal the handshake-selected values — that equality IS the mapping, no new keys — and applies POST-negotiation only: the hello request carries the version OFFER (protocol_major = adapter's major, which must equal the supervisor's major or the binding is refused since majors never negotiate; protocol_minor = adapter's preferred minor within its supported_minor offer). Deterministic selection: selected_minor = min(adapter.max, supervisor.max), valid IFF selected_minor >= max(adapter.min, supervisor.min), else no-overlap refusal with a typed version-skew failure. Every message AFTER the hello result MUST carry the selected values. Bootstrap pending scope: the hello call is keyed by (spawned-connection id + direction + wire id) with locally-pinned expected (launch target, profile, executable) — never by an assigned binding. On hello result the scope transitions atomically to the supervisor-issued binding triple. A self-reported hello naming a different target_session than launched is refused and its process terminated; hello fields never authorize target/profile. Binary bytes travel as base64-standard `*_b64` string fields, never raw newlines.

#### Scenario: Late ack after resolution is diagnostic only

- **GIVEN** a submit correlation already resolved terminal
- **WHEN** further ack evidence arrives
- **THEN** it is recorded as a diagnostic lifecycle event
- **AND** no second disposition is created

### Requirement: Common params with explicit authority binding

Every `params` object SHALL carry protocol_major u16 REQUIRED, protocol_minor u16 REQUIRED, runtime_instance string REQUIRED (supervisor-issued per supervisor start), target_session string REQUIRED, adapter_generation u64 REQUIRED (supervisor-issued, monotonic, echoed). protocol_major/minor on params MUST equal the handshake-selected values. Binding is the triple (runtime_instance, target_session, adapter_generation), validated per message even on a connection-bound transport — never inferred from generation alone. The first `adapter.handshakeHello` omits runtime_instance AND adapter_generation (both assigned in the hello result); every subsequent message carries both REQUIRED. correlation_id string: REQUIRED on `adapter.requestChoice`, `relay.look`, `adapter.submit`, and `adapter.persistState` requests (originator mints; all replies/acks echo it); OPTIONAL (absent allowed) on unsolicited `adapter.lifecycleEvent` notifications. A message missing a REQUIRED field is refused with a typed framing failure; unknown minor-version fields are ignored.

#### Scenario: Cross-instance message rejected on binding

- **GIVEN** a message with a valid generation but a foreign runtime_instance
- **WHEN** the supervisor validates binding
- **THEN** it is refused with a typed binding failure before semantic processing

#### Scenario: Overlapping minor offers select deterministically

- **GIVEN** an adapter offering supported_minor [0,2] and a supervisor supporting [1,3]
- **WHEN** version exchange completes
- **THEN** selected_minor is 2 (min of maxes, within overlap)

#### Scenario: Disjoint minor offers refuse with version skew

- **GIVEN** an adapter offering [0,0] and a supervisor supporting [1,3]
- **WHEN** version exchange completes
- **THEN** the binding is refused with a typed version-skew failure before any operation traffic

#### Scenario: Wrong-target hello is refused and terminated

- **GIVEN** an adapter launched for target Y
- **WHEN** its hello names target_session X
- **THEN** the binding is refused and its process terminated

### Requirement: Per-message payload definitions

Each method SHALL use exactly the params and result shapes defined here; deviation SHALL be a typed framing failure. `adapter.handshakeHello` request params: adapter_executable string, adapter_profile string REQUIRED closed {acp, generic} (unknown profile → handshake refusal; extension by contract revision), target_session, supported_minor {min_minor u16, max_minor u16} (single definition of the minor window lives in the negotiation table; the hello shape points at it), limits_proposal (negotiation fields below), latency_declarations map REQUIRED. Result: runtime_instance, adapter_generation assigned, selected_major u16 REQUIRED, selected_minor u16 REQUIRED (the negotiated version both sides confirm), negotiated_limits (exactly: max_frame_bytes, queue_tx_bytes, queue_rx_bytes, max_control_pending, max_events_per_second, window_seconds, max_submits_in_flight, max_choices_in_flight, max_looks_in_flight, drain_wait_ms — all post-resolution values), snapshot_b64 (current-namespace resume snapshot, empty string when none). `adapter.requestChoice` params: runtime_instance string REQUIRED, adapter_generation, correlation_id string REQUIRED (originator mints), message_id string, target_session, decider_sessions [string], acp_request_id string|u64 union OPTIONAL, ACP-profile only (opaque agent id; non-ACP adapters MUST NOT send fictitious ids — when absent, correlation_id is the sole request key), title string REQUIRED, species string REQUIRED, details Value REQUIRED, options [{option_id string, name string, species string}] non-empty (empty options are a structural violation the supervisor refuses); result: {correlation_id echoed, outcome: an externally-tagged object with exactly one key — either {"chosen": {"option_id" string REQUIRED, "decided_by" string REQUIRED}} or {"cancelled": {"decided_by"? string (absent means system/relay decided), "reason_code" namespaced string REQUIRED, "reason"? string}}; both keys present or neither present is a typed framing failure} — cancellation is a result, never a JSON-RPC error. `relay.look` params: runtime_instance string REQUIRED, look_handle_id string, adapter_generation, correlation_id string REQUIRED (originator mints), lines? u32, offset? u64, prime_timeout_ms u64; result: {correlation_id echoed, adapter_generation REQUIRED (the generation the snapshot was read from), entries[] of {kind string REQUIRED (generic closed set: text; ACP profile additionally closes user|agent|cognition|invocation|update; future profiles extend by revision; non-ACP adapters MUST use text; unknown kind for the declared profile → typed failure), seq u64 REQUIRED, text_b64 string REQUIRED (base64-standard), truncated bool REQUIRED, entry_meta? object passthrough-ignored}, entries_total u64, returned_entries_count u64, freshness closed enum fresh|stale|initializing REQUIRED, snapshot_source closed enum live|cache|synthetic|none REQUIRED (synthetic = constructed snapshots, which MUST carry stale=true, never fresh; ACP profile maps both replay-buffer and pane-capture reads onto live), stale_reason_code? string, snapshot_age_ms? u64, literal keys `stale` bool REQUIRED + `discontinuity` bool REQUIRED}. Unknown enum value is a typed framing failure. `adapter.submit` params: runtime_instance string REQUIRED, target_session string REQUIRED, correlation_id, members [{message_id, target_session, adapter_generation, content_type string REQUIRED, payload_b64}] non-empty (empty members are a structural violation the supervisor refuses), adapter_generation; watchdog latency selection: the deadline governs the submit by the MAX over members' declared latencies + margin (worst case wins; uniform-type submits reduce to the common case); unknown content_type is a structural refusal, never a default latency; result: {outcome string REQUIRED — exactly one of the literals Submitted|RefusedBusy|TransportUnavailable|SubmissionUnknown, correlation_id echoed REQUIRED, evidence_basis string REQUIRED on unknown/no-effect}. `adapter.persistState` params: runtime_instance string REQUIRED, target_session string REQUIRED, adapter_generation REQUIRED, correlation_id string REQUIRED (originator mints), tag {runtime_instance, target_session, adapter_generation}, write_id string, blob_b64 string; result: {correlation_id echoed, persisted bool REQUIRED, refusal_code on refusal ∈ {superseded-generation, retention-full} — persisted=false REQUIRES refusal_code, persisted=true PROHIBITS it (present → typed framing failure); malformed tag/blob is a validation failure, never a persist refusal} — reply sent only after commit or refusal, never on receipt alone. `adapter.lifecycleEvent` notification params use a flat `kind` discriminator key: {kind: ready|degraded|failed|progress|heartbeat REQUIRED, code string REQUIRED, detail? string}; `relay.supervisionSignal` params use a flat `signal` key: {signal: terminate|fence-hint REQUIRED}; `relay.ping` params carry only common binding fields; its result is the empty object `{}` — presence of the reply is the liveness proof, no payload. Supervision signals are hints only; enforcement is out-of-band per the supervision spec.

#### Scenario: Cancelled choice returns as result

- **GIVEN** a pending choice invalidated by respawn
- **WHEN** the relay resolves it
- **THEN** the open choice request completes with a cancelled outcome result
- **AND** no JSON-RPC error is produced

### Requirement: Handshake negotiation schema with defaults-vs-required table

Handshake SHALL negotiate limits under exactly one rule per field as defined here. REQUIRED (absent/invalid → typed negotiation refusal, binding refused; no defaults): minor window supported_minor {min_minor u16, max_minor u16}, latency_declarations map (content-type → max_ack_latency_ms u64 finite >0). NEGOTIATED-WITH-DEFAULTS (absent → default; present zero → invalid EXCEPT drain_wait_ms where 0 = fail immediately): max_frame_bytes u64 4096..67108864 default 1048576; queue_tx_bytes / queue_rx_bytes u64 default 16777216 each, zero invalid — perspective rule: each side declares its own TX/RX; effective supervisor→adapter cap = min(supervisor.queue_tx_bytes, adapter.queue_rx_bytes) and conversely (matched pairs, never same-named min by accident); max_control_pending u32 1..1024 default 32; max_events_per_second u32 1..10000 default 100 with window_seconds u32 1..60 default 1; in-flight caps max_submits_in_flight, max_choices_in_flight, max_looks_in_flight u32 default 64/16/8, zero invalid; drain_wait_ms u64 0..10000 default 250. Latency arithmetic: deadline = declared + margin, checked u64 addition (overflow → refusal), margin > 0 REQUIRED. Bootstrap: handshake_timeout_ms u64 1000..120000 default 30000 — expiry refuses the binding. Liveness: peer_liveness_timeout_ms u64 5000..600000 default 60000, reset by any valid frame; expiry routes to the fence path. Hello result gains selected_major u16 + selected_minor u16 REQUIRED (the negotiated version both sides confirm; all subsequent params must carry them). Write-id exhaustion in the CURRENT namespace: when the retained set hits max_retained_write_ids (default 1024), further persists are refused with typed persistence_retention_full until generation replacement (new namespace starts empty) — bounded memory by refusal, never silent eviction of live ids; old-generation eviction rule stands.

#### Scenario: Zero cap is invalid, not unlimited

- **GIVEN** a handshake declaring max_frame_bytes 0
- **WHEN** version exchange completes
- **THEN** the binding is refused with a typed negotiation failure naming the field

### Requirement: Event drop rules preserve authoritative state

Only `progress` and `heartbeat` kinds MAY coalesce or drop under flood. Authoritative transitions (ready, degraded, failed), ack outcomes, and fence signals MUST NEVER drop: if the control/event queue cannot admit one, the binding is fenced — admission failure escalates, never silences. Rate limiting MUST NOT manufacture healthy state: discarding a failure is impossible by construction (failures bypass the coalescible class); the supervisor retains the last authoritative transition per binding regardless of flood. Descendant escape does not defeat cessation proof: the effect-capable set includes FD holders, and cessation is observed via process exit AND FD closure under supervisor ownership — `process-group` names the mechanism, the observation invariant (no live process, no open capability FD) is the proof, and enforcement responsibility is the supervisor's in all cases.

#### Scenario: Flooded failure fences instead of vanishing

- **GIVEN** an event flood with a failed transition queued behind the cap
- **WHEN** the control queue cannot admit the failure event
- **THEN** the binding is fenced rather than continuing on a manufactured healthy state

### Requirement: Resume receipt-vs-commit linearization with bounded write-id retention

The wire SHALL distinguish frame receipt from serialized commit as defined here. Frame receipt (bytes accepted) and commit (serialized apply) are distinct stages: the `adapter.persistState` reply is sent only post-commit-or-refusal. A write received and checked under tag 7 whose commit serializes after the pointer advances to 8 gets a refused reply with the superseded code — the admitted-but-uncommitted window resolves by serialization order, never by applying to the wrong generation. Applied write_ids are retained per namespace up to max_retained_write_ids (default 1024); exhaustion evicts oldest-first ONLY for generations older than current-minus-1, and eviction is safe because tag check precedes id check (any replay of an evicted id carries a stale tag and is rejected first). Hydration encoding: snapshot_b64 in the hello result (empty = none).

#### Scenario: Evicted write-id replay is rejected by tag first

- **GIVEN** a duplicate persist whose write_id was evicted from retention
- **WHEN** it arrives tagged with a superseded generation
- **THEN** it is rejected on the tag before any id lookup
- **AND** current state stands unchanged

### Requirement: Pending-call table scopes results and errors

Each side SHALL maintain a pending-call table per supervised binding keyed by (binding triple + wire id). Wire ids are originator-monotonic counters per (binding, direction), decimal-encoded strings, starting at 1. A new call MUST carry an id greater than any previously seen from that originator in this binding; reuse is a typed framing failure with a diagnostic. Direction spaces are independent: an inbound id equal to an outbound id is unrelated, never a duplicate. u64 wrap MUST NOT occur — reaching max is a binding error: close the binding and re-handshake (unreachable in practice, defined anyway). A result or error SHALL be admitted IFF it matches an open entry with identical binding triple. Correlation vs call roles, stated once: correlation_id is OPERATION lineage (a retry is a new correlation AND a new wire id); binding triple + wire id is CALL identity for admission. Non-ACP operations key by (binding, correlation_id); calls key by (binding, wire id). A duplicate response to a resolved correlation SHALL be discarded with a diagnostic. Late frames tagged with a superseded generation (results, errors, and notifications alike) SHALL be discarded with a diagnostic per the stale rule. Entries SHALL clear on terminal response, fence, generation replacement, or binding refusal. No reliance on bare ids or caller-asserted deciders anywhere in this path.

#### Scenario: Correlation-invalid error admitted by table match

- **GIVEN** an open choice entry with binding triple T and wire id w
- **WHEN** a contract error reporting correlation_invalid arrives for (T, w) — parseable, names its target, rejects its own correlation field
- **THEN** it is admitted by table match and resolves the entry
- **AND** a second response to w is discarded as diagnostic

#### Scenario: Unparseable frames never resolve entries

- **GIVEN** an open entry with binding triple T and wire id w
- **WHEN** an unparseable frame arrives (e.g. -32700: no id, no (T, w) nameable)
- **THEN** it is logged with a diagnostic and resolves nothing

#### Scenario: Binding-wide fence terminalizes per call type

- **GIVEN** open entries under binding T: a submit, a choice, a look, a persist, and a ping
- **WHEN** a fence-hint supervisionSignal notification names T (notifications carry no wire id)
- **THEN** admitted by binding-T match alone
- **AND** each entry resolves per its type: the submit per the trichotomy evidence rule (unknown unless its bytes demonstrably never left the outbox, outcome carrying its evidence basis); the choice resolves adapter-local Cancelled with relay invalidation via the dual-cancel path; the look resolves with a synthetic stale EMPTY snapshot (source=synthetic, stale=true, entries=[], zero counts, adapter_generation of the fencing generation); the persist follows its serialized commit outcome; the ping is dropped with no terminal (liveness only)
- **AND** the hint starts terminalization only: it is NOT observed cessation and does NOT release held single-flight slots (a watchdog-held slot stays held until cessation is observed) or authorize replacement — those require observed cessation per the cessation requirement
- **AND** supervisor-side observation (liveness expiry, process exit, FD closure) triggers identical per-type terminalization with NO wire receipt required — wire delivery is never a precondition for any terminalization

#### Scenario: Resolve, reuse refusal, and late old reply

- **GIVEN** request w resolved terminal, then a new request reusing w, then a late old reply w
- **WHEN** the new request arrives and then the old reply arrives
- **THEN** the reused w is refused as a typed framing failure
- **AND** the late old reply is discarded as diagnostic with no open entry

### Requirement: Non-ACP choices route without ACP ids

A choice request without acp_request_id SHALL route and resolve on correlation_id alone. Non-ACP adapters SHALL NOT send fictitious ACP ids.

#### Scenario: Non-ACP choice resolves on correlation alone

- **GIVEN** a choice request with no acp_request_id from a non-ACP adapter
- **WHEN** the relay resolves it
- **THEN** the outcome correlates by correlation_id with no ACP id involved
