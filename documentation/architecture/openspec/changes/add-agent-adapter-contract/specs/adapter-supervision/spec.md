## ADDED Requirements

Ownership: Backend lane. This file records the agreed interface constraints binding both sides; the supervisor design is Backend-authored.

### Requirement: Supervisors own out-of-band termination and cessation

The supervisor SHALL own out-of-band termination and observable cessation of the entire owned effect-capable execution set, including descendants and file-descriptor holders. Unreachable SHALL NOT count as ceased. A negative fence SHALL fail stop. Terminate/observe RPC shapes are optional detail; shutdown, detach, and adoption choices SHALL NOT be inherited as fixed.

#### Scenario: Wedged adapter fenced out-of-band

- **WHEN** an adapter stops responding to terminate/observe RPCs
- **THEN** the supervisor terminates the owned execution set out-of-band and observes cessation without relying on the adapter's cooperation

### Requirement: Launch and wire failures are typed

Spawn refusal, handshake timeout, protocol skew, and unknown-generation failures SHALL cross the wire as typed failures in the `TransportError` shape (`code`, `reason`, optional `details`), with relay-authored and adapter-authored codes composed by namespace, never collided. Launch failures — where no child wire exists yet — SHALL be originated locally by the supervisor through the runtime-facing surface in the same typed shape, never received over the wire.

#### Scenario: Spawn refusal typed

- **WHEN** an adapter executable cannot be launched
- **THEN** the supervisor originates a typed launch failure locally, distinguishing permanent (missing binary, denied exec) from transient causes

### Requirement: No back-edge from adapter to relay internals

The adapter SHALL hold opaque handles over the wire and SHALL NOT import relay internals. Call bounding follows a per-shape matrix, never a uniform rule over every blocked call. Bounded shapes the relay deadlines: control signals (cancellation, fence signal/observe, ack admission — in whatever shape each is defined; terminate/observe RPC shapes stay optional per the fence contract, and fencing MUST NOT depend on cooperative wire calls) and the Look RPC (outer deadline is the relay awaiting the adapter — strictly greater than the adapter prime timeout — retiring the correlation and distinguishing no snapshot from stale cached output beyond it). Explicitly out of the bounded set: human-paced choice waits, which follow the explicit waiting policy in the choice-rpc spec. Backpressure limits (frame/queue bounds) SHALL be explicit and interoperable through negotiation and local enforcement — not necessarily identical hardcoded sizes on both sides.

#### Scenario: Hung adapter does not hang relay look

- **WHEN** an adapter exceeds the relay's outer look deadline
- **THEN** the relay retires the correlation and reports no snapshot — never a stale cached snapshot presented as fresh, and never a block on the adapter

### Requirement: Per-attempt submission watchdog bounds withholding acks

For every submit with an open per-attempt correlation, the supervisor SHALL enforce a watchdog deadline sourced from negotiated configuration. If the adapter stays connected but withholds its ack past the deadline, the supervisor SHALL resolve `SubmissionUnknown` on the watchdog trigger — never on absence-of-ack alone while the attempt is young — with no blind replay; retry requires an explicit new submission with a new correlation. The watchdog deadline SHALL exceed the adapter's documented max ack latency for the submitted content type, so a healthy slow ack never races the trigger.

#### Scenario: Connected-but-silent adapter resolves unknown on watchdog

- **GIVEN** a submit with an open per-attempt correlation and a connected adapter
- **WHEN** no ack arrives before the watchdog deadline
- **THEN** the supervisor resolves `SubmissionUnknown` on the trigger
- **AND** does not resubmit automatically

### Requirement: Normative backpressure bounds with typed refusal

The supervisor SHALL enforce per-adapter in-flight bounds: maximum concurrent submits awaiting ack, maximum pending choice resolutions, maximum concurrent look calls. Each bound SHALL be a negotiated handshake limit resolved down to the lesser of both sides. An operation arriving beyond its bound SHALL be refused immediately with a typed relay-authored backpressure failure naming the exhausted bound — never queued unboundedly, never silently dropped. Hitting a bound is abnormal and inscribed.

#### Scenario: Submit beyond the in-flight bound is refused

- **GIVEN** an adapter with all submit-ack slots occupied
- **WHEN** another submit arrives
- **THEN** the supervisor refuses it at once with a typed backpressure failure naming the submit-ack bound
- **AND** no queue grows behind the refusal

### Requirement: Declared packing schema with supervisor-side membership validation

Every submit SHALL declare its packing membership: the exact member list, the adapter generation binding, and one correlation shared by all members. The supervisor SHALL validate the declaration before forwarding: undeclared members are rejected with a typed failure, and one ack outcome answers for exactly the declared members and none outside them. A submit carrying members from more than one target SHALL be rejected — packing is per-target under the single-target scope below.

#### Scenario: Undeclared member rejected before forwarding

- **GIVEN** a submit declaring members A and B
- **WHEN** the framed write would also carry C
- **THEN** the supervisor rejects the submit with a typed membership failure before anything is forwarded

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

### Requirement: Generation-tagged replacement fences resume writes; currency at relay, bytes at adapter

Resume-state writes SHALL carry the relay-issued (target, generation) tag. Only writes tagged with the relay-held current generation SHALL take effect; writes under a superseded tag SHALL be rejected and SHALL NOT alter standing state. The relay SHALL hold canonical target identity and generation currency and SHALL never parse resume blobs; the adapter SHALL persist opaque blobs keyed by tag. No CAS loop, no advisory lock — the tag comparison is the fence.

#### Scenario: Superseded generation write fenced by tag

- **GIVEN** generation 7 superseded by generation 8 for a target
- **WHEN** a write tagged (target, 7) arrives
- **THEN** it is rejected and generation 8's state stands

### Requirement: Version skew refused by major equality with negotiated windows

Adapter and supervisor SHALL each declare a supported protocol major/minor range and framing limits at handshake. Binding SHALL require major-version equality; minor versions SHALL interoperate within the overlap of both declared windows, with framing limits resolved down to the lesser of both sides. An adapter outside the overlap SHALL be refused with a typed version-skew failure before any operation traffic — never operated degraded.

#### Scenario: Major mismatch refused before operations

- **GIVEN** an adapter declaring a protocol major outside the supervisor's supported range
- **WHEN** version exchange completes
- **THEN** the supervisor refuses the binding with a typed version-skew failure
- **AND** no choice, look, or submit traffic flows

### Requirement: Lifecycle event schema and ownership split

Adapter-to-relay lifecycle events (ready, degraded, failed) and relay-to-adapter signals (terminate, fence) SHALL share one envelope: adapter generation binding, event type, namespaced typed code, optional details. The supervisor SHALL own the adapter process lifecycle, the effect-capable execution set, fencing, and every deadline it enforces; the adapter SHALL own the agent child, the ordered ACP lifecycle, prime-waits, and single-flight gates. Neither side SHALL enforce the other's owned timers: the relay passes timeout values, the adapter runs its waits, and each expiry is reported as an event, never assumed by the peer.

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
