## ADDED Requirements

### Requirement: Wire framing and typed errors

The adapter SHALL read newline-delimited UTF-8 JSON-RPC 2.0 frames over stdio and enforce two inbound bounds: the 64KiB bootstrap ceiling before negotiation completes, and the negotiated `max_frame_bytes` as the operative bound after — refusing once the bound is exceeded without allocating a body-sized buffer. Stdout carries complete frames only; diagnostics go elsewhere. An oversize or unparseable frame has no usable id or correlation, so the adapter SHALL discard it with a local diagnostic and never answer it with a correlated `-32000` carrying an invented correlation.

#### Scenario: Oversize inbound frame refused without body allocation

- **WHEN** inbound bytes for one frame exceed the operative bound before any terminating newline
- **THEN** the adapter refuses with a typed framing failure without having allocated a body-sized buffer

#### Scenario: Unparseable frame resolves nothing

- **WHEN** an unparseable frame arrives with no nameable id or correlation
- **THEN** the adapter logs a diagnostic, resolves no pending entry, and emits no correlated error

### Requirement: Error taxonomy and correlation discipline

Contract failures SHALL use JSON-RPC error code `-32000` with `data: {code, reason, details?, correlation_id?}`, echoing correlation_id when the failed request carried one — EXCEPT `correlation_invalid`, which is exempt from the required echo and is admitted by pending-table match instead. Standard JSON-RPC parse and protocol errors carry no data-correlation requirement. On an unusable channel no wire reply is required or sent; terminal outcomes surface locally only.

#### Scenario: Contract error echoes correlation

- **WHEN** a submit with correlation_id C fails adapter-side validation
- **THEN** the error carries code -32000 with data.correlation_id C

#### Scenario: correlation_invalid admitted without required echo

- **WHEN** a contract error reporting correlation_invalid arrives for an open entry
- **THEN** it is admitted by table match with no required correlation echo

#### Scenario: Dead channel sends nothing

- **WHEN** the transport is unusable when a terminal outcome resolves
- **THEN** the adapter emits no wire reply and records the outcome locally

### Requirement: Handshake negotiation

The adapter SHALL complete `handshakeHello` with major equality plus minor-window overlap (selected_minor = min of maxes, valid only within the overlap), negotiated limits resolved per the contract table with matched pairs — effective supervisor→adapter cap = min(supervisor.queue_tx_bytes, adapter.queue_rx_bytes), never same-named min by accident — mandatory `latency_declarations`, and deterministic refusal (typed version-skew / validation failure, no degraded operation) on any mismatch.

#### Scenario: Major mismatch refused before traffic

- **WHEN** the supervisor's selected major differs from the adapter's supported range
- **THEN** the adapter reports the skew and no submit/choice/look traffic flows

#### Scenario: Missing latency declaration refused

- **WHEN** the adapter's own script omits latency_declarations
- **THEN** binding is refused at handshake time, before any operation traffic

### Requirement: Submit execution and ack trichotomy

The adapter SHALL enforce single-flight (one turn in flight; further submits get `RefusedBusy` naming adapter single-flight, never relay refusal), execute the scripted turn, and resolve exactly one terminal per correlation using the contract outcome literals: `Submitted` on observed submission (transport_ack evidence), `RefusedBusy` as no-effect evidence, `TransportUnavailable` ONLY for provable non-submission (serialization failure, active-prompt refusal with demonstrably unbuffered bytes, or no runtime), and `SubmissionUnknown` otherwise (adapter-side write failure without proof that zero bytes left). Unknown and no-effect outcomes SHALL carry `evidence_basis` and echo correlation_id.

#### Scenario: Busy submit refused as no-effect

- **WHEN** a submit arrives while a turn is in flight
- **THEN** the reply is `RefusedBusy` and nothing is queued

#### Scenario: Failed write without proof resolves unknown with evidence

- **WHEN** the scripted agent write fails without proof that zero bytes left
- **THEN** the reply is `SubmissionUnknown` carrying `evidence_basis` and the echoed correlation_id, never success

#### Scenario: Provable non-submission is TransportUnavailable

- **WHEN** serialization of the agent write fails with demonstrably unbuffered bytes
- **THEN** the reply is `TransportUnavailable` carrying `evidence_basis`, never `SubmissionUnknown`

### Requirement: Choice raising and dual cancel

The adapter SHALL raise `requestChoice` with generation binding + correlation, block only its own turn resolver (cancel/fence/ack/output proceed concurrently), record the outcome before answering the agent, and cancel its local resolvers on disconnect without waiting for the wire.

#### Scenario: Disconnect cancels locally

- **WHEN** the transport drops while a choice is pending
- **THEN** the adapter cancels its local resolver immediately

#### Scenario: Fence preempts pending choice

- **WHEN** fence signaling begins while a choice is pending
- **THEN** fence observation proceeds without waiting for the choice

### Requirement: Bootstrap establishment

The adapter SHALL originate `handshakeHello` keyed by (spawned-connection id + direction + wire id) with locally-pinned expected launch inputs (target, profile, executable), never by an assigned binding. On the hello result it SHALL establish scope atomically from the captured bootstrap pending context: the result carries no target_session and no triple to validate against — instead the adapter validates the result is well-formed (assigned runtime_instance and adapter_generation present, selected versions within the offered window, limits sane) against its own offer and pinned launch inputs (target, profile, executable, and its own hello request's target_session). It SHALL refuse the binding with termination (exit, no traffic) on a malformed result or an out-of-window selection. Bootstrap establishment is not validation against preassigned runtime/generation — the assignment arrives in the result.

#### Scenario: Out-of-window selection terminates

- **WHEN** the hello result selects versions outside the offered window
- **THEN** the adapter refuses the binding and emits no submit/choice/look/persist traffic

#### Scenario: Malformed result terminates

- **WHEN** the hello result omits the assigned runtime_instance or adapter_generation
- **THEN** the adapter refuses the binding with termination

#### Scenario: Valid result establishes captured binding

- **WHEN** the hello result matches offer and launch inputs
- **THEN** the adapter captures the binding triple and all subsequent traffic uses it

### Requirement: Post-bind call discipline

After binding, requests and notifications SHALL carry the full triple, validated per message; results and errors SHALL carry no triple and are admitted only by pending-table match on captured binding plus direction plus wire id. Wire ids are originator-monotonic from 1 per (binding, direction) as decimal strings with no reuse (reuse is a typed framing failure; u64 wrap closes the binding); correlation_id is operation lineage only, where a retry is a new correlation AND a new wire id; direction spaces are independent. Duplicate or stale replies are discarded with a diagnostic; a reply for an unknown wire id is discarded with a local diagnostic and never answered with a JSON-RPC error (no reply-to-reply loop).

#### Scenario: Triple-free result admitted by table

- **WHEN** a choice result arrives carrying no binding triple but matching an open entry's binding, direction, and wire id
- **THEN** the adapter admits and resolves it

#### Scenario: Unknown reply discarded silently on the wire

- **WHEN** a reply arrives for a wire id with no open entry
- **THEN** the adapter records a local diagnostic and emits no wire error in reply

#### Scenario: Reused wire id refused

- **WHEN** a new call reuses an already-seen wire id from the same originator
- **THEN** the adapter refuses it as a typed framing failure

### Requirement: Look poll responses

The adapter SHALL answer `relay.look` polls with snapshot entries carrying full freshness metadata, the adapter generation the snapshot was read from, and adapter-side prime-wait bounded strictly by the relay-supplied timeout (zero means no wait); synthetic marks carry stale=true, never fresh.

#### Scenario: Prime-wait bounded by relay timeout

- **WHEN** a poll arrives for an initializing target with nonzero prime timeout
- **THEN** the adapter waits up to that timeout, then returns a stale-tagged result rather than blocking

### Requirement: Persist and lifecycle emission

The adapter SHALL submit `persistState` writes tagged with its current generation (never checking currency itself), and emit `lifecycleEvent`s (ready/degraded/failed) with namespaced typed codes without dictating supervision outcomes.

#### Scenario: Degraded reported with code

- **WHEN** the scripted agent degrades
- **THEN** the adapter emits a degraded event with a typed code and continues under supervisor policy

### Requirement: Frozen-contract normativity

The adapter SHALL implement every adapter-side obligation of the frozen contract; where this spec restates a subset, the contract text governs on any divergence. The restated subset SHALL NOT be read as narrowing the normative contract.

#### Scenario: Unrestated adapter-side rule follows contract

- **WHEN** the adapter emits lifecycle events under flood
- **THEN** it applies the contract's coalescing classes (progress/heartbeat may coalesce; authoritative never drops) whether or not this spec repeats them

### Requirement: Adapter outbox bounds and control priority

The adapter SHALL enforce its declared queue bounds with the perspective rule (its TX against the peer's RX), block data writes only up to the negotiated bounded drain wait and then fail rather than queue unboundedly, carry control traffic (fence, cancel, terminate, ack admission) on a priority lane exempt from data caps within the negotiated control bound, coalesce or drop ONLY progress/heartbeat events under flood while never dropping authoritative transitions (ready, degraded, failed), ack outcomes, or fence signals, retain the last authoritative transition regardless of flood, and fence on control/event admission failure rather than silence.

#### Scenario: Flooded authoritative event fences instead of dropping

- **WHEN** the event queue cannot admit a degraded event under flood
- **THEN** the binding fences rather than dropping the event or manufacturing healthy state

### Requirement: Adapter-originated in-flight caps

The adapter SHALL NOT exceed the negotiated in-flight caps for originated `requestChoice` calls (within max_choices_in_flight), shedding further choice needs locally with a typed internal refusal inscribed with its code to the local diagnostic sink (structured log / fixture-visible record, never stdout) rather than queuing unboundedly. Originated `persistState` writes are bounded by an explicit adapter-local finite cap on outstanding persist entries plus the byte bounds — the frozen hello carries no persist-specific in-flight cap, so none is assumed. Inbound submits are governed by the single-flight turn gate; inbound look polls proceed concurrently within max_looks_in_flight, and control traffic proceeds on the priority lane — the single-flight gate limits turns, not Look/control progress during a pending turn or choice.

#### Scenario: Choice cap sheds locally with inscription

- **WHEN** outstanding originated choices reach the negotiated cap
- **THEN** a further choice need is shed with a typed internal refusal inscribed with its code to the local diagnostic sink, never queued unboundedly

#### Scenario: Look proceeds during pending turn

- **WHEN** a turn is in flight and a look poll arrives within max_looks_in_flight
- **THEN** the poll is answered without waiting for the turn, and no stale snapshot is presented as fresh

### Requirement: Ping liveness

The adapter SHALL answer `relay.ping` requests with the empty-object result, reset its liveness tracking on any valid frame, and on liveness expiry follow the same local-cancel path as a disconnect (cancel resolvers, invalidate pending entries).

#### Scenario: Ping answered with empty result

- **WHEN** a `relay.ping` request arrives on a live binding
- **THEN** the adapter replies `{}` and the reply carries no payload

### Requirement: Cancellation and pending-call cleanup

On disconnect, fence, generation replacement, or binding refusal, the adapter SHALL terminalize locally: cancel local resolvers and record each operation's known terminal outcome BEFORE retiring its pending-table entry — submit keeps its trichotomy evidence outcome, persist keeps its serialized commit or refusal outcome, choice resolves local Cancelled, look resolves a synthetic stale empty snapshot, ping is dropped with no terminal. Retired entries never admit replies and wire ids are never reused across the cleanup. Adapter-side "fence" means local terminalization plus handing observation to the supervisor (lifecycle event while the pipe is usable, then the effect set terminates by supervisor observation) — never adapter-owned cessation proof and never publishing a replacement. No wire response is required or sent after disconnect; a persist whose reply was lost is never recorded as persisted=true without commit proof; clearing pending entries never releases an unknown turn's single-flight gate (the gate releases only on observed cessation of the attempt's writer).

#### Scenario: Fence clears pending state without reuse

- **WHEN** a fence clears three open entries and new calls originate afterwards
- **THEN** the new calls mint fresh wire ids and no cleared entry admits a reply

#### Scenario: Submit evidence preserved across cleanup

- **WHEN** a fence arrives with a submit whose bytes demonstrably left the outbox
- **THEN** the recorded outcome is SubmissionUnknown with evidence_basis before the entry retires

#### Scenario: Lost persist reply never fabricates success

- **WHEN** a persist commit's reply is lost on disconnect
- **THEN** the adapter does not record persisted=true; the outcome stays unknown pending re-establishment

### Requirement: Determinism

Wire-visible behavior SHALL be a pure function of an explicit input trace: received frames, fixture script, supervisor-supplied deadlines, fixture-driven virtual-clock advances, and deterministic dispatch ordering. Timeout expiry is driven only by virtual-clock advance events in the trace — a duration alone expires nothing. No randomness exists in any wire-decision path (the fixture script is the only source of variation); no `SystemTime`, ambient randomness, or thread-scheduling observation. For the executable, clock events arrive through the injected clock (virtual in replay/test, monotonic source live); replay guarantees cover identical traces, not external IO scheduling. Scripted `transport_ack` evidence models a fake agent effect boundary and is not proof that any real agent accepted input.

#### Scenario: Transcript replay is byte-identical

- **WHEN** the same input trace (frames + script + clock advances + dispatch order) replays twice through the adapter
- **THEN** both emitted transcripts are byte-identical

#### Scenario: Timeout without clock advance does not fire

- **WHEN** a prime-wait deadline passes in wall-clock terms with no virtual-clock advance in the trace
- **THEN** no timeout outcome is emitted
