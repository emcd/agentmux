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
