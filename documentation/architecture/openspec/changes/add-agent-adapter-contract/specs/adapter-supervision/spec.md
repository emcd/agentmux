## ADDED Requirements

Ownership: Backend lane. This file records the agreed interface constraints binding both sides; the supervisor design is Backend-authored.

### Requirement: Supervisors own out-of-band termination and cessation

The supervisor SHALL own out-of-band termination and observable cessation of the entire owned effect-capable execution set, including descendants and file-descriptor holders. Unreachable SHALL NOT count as ceased. A negative fence SHALL fail stop. Terminate/observe RPC shapes are optional detail; shutdown, detach, and adoption choices SHALL NOT be inherited as fixed.

#### Scenario: Wedged adapter fenced out-of-band

- **WHEN** an adapter stops responding to terminate/observe RPCs
- **THEN** the supervisor terminates the owned execution set out-of-band and observes cessation without relying on the adapter's cooperation

### Requirement: Launch and wire failures are typed

Handshake timeout, protocol skew, and unknown-generation failures SHALL cross the wire as typed failures in the `TransportError` shape (`code`, `reason`, optional `details`), with relay-authored and adapter-authored codes composed by namespace, never collided. Launch failures — where no child wire exists yet — SHALL be originated locally by the supervisor through the runtime-facing surface in the same typed shape, never received over the wire.

#### Scenario: Spawn refusal typed

- **WHEN** an adapter executable cannot be launched
- **THEN** the supervisor originates a typed launch failure locally, distinguishing permanent (missing binary, denied exec) from transient causes

### Requirement: No back-edge from adapter to relay internals

The adapter SHALL hold opaque handles over the wire and SHALL NOT import relay internals. Call bounding follows a per-shape matrix, never a uniform rule over every blocked call. Bounded shapes the relay deadlines: control signals (cancellation, fence signal/observe, ack admission — in whatever shape each is defined; terminate/observe RPC shapes stay optional per the fence contract, and fencing MUST NOT depend on cooperative wire calls) and the Look RPC (outer deadline is the relay awaiting the adapter — strictly greater than the adapter prime timeout — retiring the correlation and distinguishing no snapshot from stale cached output beyond it). Explicitly out of the bounded set: human-paced choice waits, which follow the explicit waiting policy in the choice-rpc spec. Backpressure limits (frame/queue bounds) SHALL be explicit and interoperable through negotiation and local enforcement — not necessarily identical hardcoded sizes on both sides.

#### Scenario: Hung adapter does not hang relay look

- **WHEN** an adapter exceeds the relay's outer look deadline
- **THEN** the relay retires the correlation and reports no snapshot — never a stale cached snapshot presented as fresh, and never a block on the adapter

### Requirement: Per-attempt submission watchdog bounds withholding acks

For every submit with an open per-attempt correlation, the supervisor SHALL enforce a watchdog deadline sourced from negotiated configuration. The handshake `latency_declarations` SHALL publish max ack latency per content type as a map of content-type to max_ack_latency_ms (u64, required, finite, greater than 0). Supervisor deployment config SHALL carry `watchdog_margin_ms` and `max_acceptable_latency_ms`. The deadline SHALL equal declared latency plus margin, with the timer originating at the framed-write dispatch timestamp on the supervisor clock. Missing, invalid, unbounded, or above-max declarations SHALL refuse the binding with a typed `validation_latency_declaration` failure — strict, no silent defaults. If the adapter stays connected but withholds its ack past the deadline, the supervisor SHALL resolve `SubmissionUnknown` on the watchdog trigger — never on absence-of-ack alone while the attempt is young — with no blind replay; retry requires an explicit new submission with a new correlation.

#### Scenario: Connected-but-silent adapter resolves unknown on watchdog

- **GIVEN** a submit with an open per-attempt correlation and a connected adapter
- **WHEN** no ack arrives before the watchdog deadline
- **THEN** the supervisor resolves `SubmissionUnknown` on the trigger
- **AND** does not resubmit automatically

#### Scenario: Missing latency declaration refuses binding

- **GIVEN** an adapter omitting latency_declarations at handshake
- **WHEN** version exchange completes
- **THEN** the supervisor refuses the binding with a typed validation failure before any operation traffic

### Requirement: Normative backpressure bounds with typed refusal

The supervisor SHALL enforce per-adapter in-flight bounds: maximum concurrent submits awaiting ack, maximum pending choice resolutions, maximum concurrent look calls. Each bound SHALL be a negotiated handshake limit resolved down to the lesser of both sides. An operation arriving beyond its bound SHALL be refused immediately with a typed relay-authored backpressure failure naming the exhausted bound — never queued unboundedly, never silently dropped. Hitting a bound is abnormal and inscribed.

#### Scenario: Submit beyond the in-flight bound is refused

- **GIVEN** an adapter with all submit-ack slots occupied
- **WHEN** another submit arrives
- **THEN** the supervisor refuses it at once with a typed backpressure failure naming the submit-ack bound
- **AND** no queue grows behind the refusal

### Requirement: Framing and queue-byte enforcement with control priority

Handshake SHALL negotiate max frame bytes and per-direction queue-byte caps, resolved down to the lesser of both sides, within hardcoded bootstrap limits that bound the handshake itself: fixed conservative defaults apply until negotiation completes (max frame 64KiB, control pending 8). Field types, ranges, defaults, and invalid-value behavior are defined once, in the wire-schema negotiation table, which is the single source of truth — this paragraph states no field list of its own. Data writes beyond the queue-byte cap SHALL fail with a typed backpressure failure — writers block only up to a bounded drain wait, then fail rather than queue unboundedly. Control traffic (fence, cancel, terminate, ack resolution) SHALL travel a priority lane exempt from data caps but carrying its own control bound: the wire lane is optimization only — fence and termination enforcement never wait for wire delivery and proceed out-of-band (process-group) when the pipe is blocked and the peer unresponsive. Event flood is bounded by max_events_per_second (u32, default 100, negotiated); beyond cap the supervisor discards with a counter carried in the next admitted event plus a typed slow-down signal — never silent. A full pipe SHALL terminalize pending data attempts with typed failures rather than stall them indefinitely; a terminalized attempt resolves unknown unless the bytes demonstrably never left the supervisor outbox (then no-effect), the outcome carries its evidence basis, and unknown never implies safe retry (new correlation required). Unusable channel: no wire error is required; terminal outcomes surface through the runtime observation boundary marked locally-synthesized with error.data.synthesized_by='supervisor', and MUST NEVER be fabricated as peer frames (wire-schema references this rule for terminal outcomes without a usable channel).

#### Scenario: Fence preempts a full data pipe

- **GIVEN** an adapter with data queues at cap and a pending fence
- **WHEN** fence signaling begins
- **THEN** fence and cessation observation proceed out-of-band regardless of wire deliverability
- **AND** pending data attempts terminalize with typed backpressure failures carrying their evidence basis

### Requirement: Declared packing schema with supervisor-side membership validation

Every submit SHALL declare its packing membership: the exact member list, the adapter generation binding, and one correlation shared by all members. Member identity per member is (message_id, target_session, adapter_generation, payload_b64) per the wire schema. The supervisor SHALL validate the declaration before forwarding: undeclared members are rejected with a typed failure, and one ack outcome answers for exactly the declared members and none outside them. The supervisor SHALL structurally validate each member before forwarding — target equals the bound target, adapter_generation equals current, correlation open — and SHALL reject the whole submit with a typed membership failure on any mismatch; partial forwarding SHALL NOT occur. A submit carrying members from more than one target SHALL be rejected — packing is per-target under the single-target scope below.

#### Scenario: Undeclared member rejected before forwarding

- **GIVEN** a submit declaring members A and B
- **WHEN** the framed write would also carry C
- **THEN** the supervisor rejects the submit with a typed membership failure before anything is forwarded

#### Scenario: Generation-mismatched member rejects the whole submit

- **GIVEN** a submit with one member bound to a superseded generation
- **WHEN** the supervisor validates packing membership
- **THEN** the whole submit is rejected with a typed membership failure
- **AND** no member is forwarded

### Requirement: Human-paced choice waits are unbounded-but-invalidatable with an operator ceiling

Human-paced choice waits SHALL NOT carry a fixed protocol deadline. A pending `request_choice` SHALL resolve only by decision, by `Cancelled` on shutdown / respawn-invalidation / transport disconnect / explicit cancel, or by `Cancelled` with a wait-ceiling code when an operator-configured maximum wait elapses. The ceiling SHALL be deployment configuration, stated separately from bounded control/look RPC deadlines; its expiry SHALL cancel locally at the adapter and invalidate relay-side through the same dual-cancel path as disconnect.

#### Scenario: Wait ceiling cancels both sides

- **GIVEN** a pending choice with an operator-configured maximum wait
- **WHEN** the ceiling elapses with no decision
- **THEN** the adapter cancels its local resolver and the relay invalidates the pending choice
- **AND** the turn observes `Cancelled` with the wait-ceiling code, never a transport error

### Requirement: Single-target adapter scope for the initial contract

Each adapter instance SHALL serve exactly one target session, declared at launch and confirmed at handshake. The supervisor SHALL enforce the binding: traffic naming any other target through that instance is rejected with a typed scope failure. Multi-target adapters are explicitly out of this contract revision; adding them requires a contract revision with its own scope, generation-binding, and authorization rules — scope SHALL NOT be inferred from generation or traffic shape.

#### Scenario: Cross-target traffic through a bound instance is rejected

- **GIVEN** an adapter instance bound to target A
- **WHEN** a request names target B through that instance
- **THEN** the supervisor rejects it with a typed scope failure

### Requirement: Cessation-before-replacement with immutable per-generation persistence

No replacement generation SHALL be published until the superseded generation's cessation is supervisor-observed out-of-band over the effect-capable set (descendants and FD holders; unreachable is not ceased) — never adapter-declared cooperation. Each generation SHALL persist resume state into its own immutable namespace and SHALL never mutate another generation's blobs. Commit/selection authority is the supervisor, exclusively: admission check (presented tag equals supervisor-held current, under supervisor serialization) and commit (apply write / advance pointer) execute as one supervisor-serialized step. Adapters submit writes; they never check currency and never touch the current pointer — from any adapter's perspective there is no check-then-act gap because the adapter performs no check. Wire tag comparison is defense-in-depth, not the enforcement. On supervisor restart, currency SHALL start empty via a fresh runtime-instance identity issued at supervisor start: tags carry the instance id, a new instance never matches old tags, and no durable store is mandated.

Single-flight slot release requires observed cessation of the attempt's writer (via fence) or terminal ack/no-effect resolution. Correlation retirement prevents reply admission only — it does NOT stop late external writes and does NOT release the slot. A watchdog-triggered unknown keeps the slot held; fresh submits are refused as busy until cessation is observed.

The supervisor SHALL own the resume store bytes; the opaque-handle distinction survives (the relay never parses blobs), only byte custody moves, because single-commit-authority extends to the store and nowhere else. Wire interface, two RPCs: `persist_state(tag, write_id, blob)` adapter-to-supervisor (admitted iff tag equals current under supervisor serialization; applied write_ids tracked per namespace, bounded, duplicates rejected); hydration served BY the supervisor TO the new generation at bind. Import is an explicit supervisor-driven step in the publish sequence (inscribed, auditable, never silent) within-instance; cross-restart import is explicitly NOT supported (fresh instance identity) — acknowledged, not a gap.

#### Scenario: Check-versus-replacement race resolves deterministically

- **GIVEN** a write admitted under tag 7 while 7 is current
- **WHEN** cessation of 7 is observed, the pointer advances to 8, and a duplicate write tagged 7 arrives late
- **THEN** the late write is rejected by tag
- **AND** generation 8 stands unchanged
- **AND** the rejection carries a typed superseded-generation code

#### Scenario: Restart starts with empty currency

- **GIVEN** a supervisor restart with prior adapter processes terminated
- **WHEN** a resume load is attempted before any fresh generation publishes
- **THEN** no stale generation's state is readable
- **AND** the load reports no-current-generation distinctly from a fence rejection

#### Scenario: Admitted-but-uncommitted write racing replacement is refused

- **GIVEN** a write admitted under tag 7 but not yet committed
- **WHEN** the pointer advances to 8 before the commit executes
- **THEN** the commit is REFUSED with a typed superseded-generation code
- **AND** already-committed generation-7 bytes stay historical and never become selected current

#### Scenario: Duplicate persist write rejected by id

- **GIVEN** a persist_state write already applied under its write_id
- **WHEN** the same write_id arrives again
- **THEN** it is rejected as a duplicate without altering state

### Requirement: Version skew refused by major equality with negotiated windows

Adapter and supervisor SHALL each declare a supported protocol major/minor range and framing limits at handshake. Binding SHALL require major-version equality; minor versions SHALL interoperate within the overlap of both declared windows, with framing limits resolved down to the lesser of both sides. An adapter outside the overlap SHALL be refused with a typed version-skew failure before any operation traffic — never operated degraded.

#### Scenario: Major mismatch refused before operations

- **GIVEN** an adapter declaring a protocol major outside the supervisor's supported range
- **WHEN** version exchange completes
- **THEN** the supervisor refuses the binding with a typed version-skew failure
- **AND** no choice, look, or submit traffic flows

### Requirement: Lifecycle event schema and ownership split

Adapter-to-relay lifecycle events (ready, degraded, failed) and relay-to-adapter signals (terminate, fence) SHALL share one envelope: adapter generation binding, event type, namespaced typed code, optional details. The supervisor SHALL own the adapter process lifecycle, the effect-capable execution set, fencing, and every deadline it enforces; the adapter SHALL own the agent child and — for the ACP adapter profile — the ordered ACP lifecycle, prime-waits, and single-flight gates. Neither side SHALL enforce the other's owned timers: the relay passes timeout values, the adapter runs its waits, and each expiry is reported as an event, never assumed by the peer. This SHALL NOT limit the supervisor's independent watchdog and outer-deadline enforcement, which resolve without awaiting peer expiry events.

#### Scenario: Degraded adapter reports, supervisor decides

- **WHEN** an adapter reports a degraded lifecycle event with a typed code
- **THEN** the supervisor records the event and applies its own supervision policy (continue, fence, or terminate) without the adapter dictating the outcome

### Requirement: Relay-spawned adapters; no self-registration or orphan adoption

The supervisor SHALL launch every adapter process it supervises; adapters SHALL NOT self-register. On supervisor restart, prior adapter processes SHALL be terminated out-of-band and relaunched, never re-bound. Detach (an adapter running past supervision) SHALL NOT exist in this contract revision.

#### Scenario: Restart relaunches rather than adopts

- **GIVEN** a supervisor restart with a prior adapter process still alive
- **WHEN** the new supervisor starts
- **THEN** it terminates the orphan out-of-band and launches a fresh adapter
- **AND** no traffic flows under the orphan's generation
