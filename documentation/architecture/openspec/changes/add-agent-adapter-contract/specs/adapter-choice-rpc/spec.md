## ADDED Requirements

### Requirement: Adapter requests choices over a blocking reverse-RPC

The system SHALL provide an adapter→relay `request_choice` reverse-RPC that blocks the calling turn resolver until resolved. The request SHALL carry the adapter generation id on every call; `request_id` alone SHALL NOT route. The `epoch` field is reserved and SHALL carry no meaning until the joint freeze defines it. The request SHALL carry the transport-native `request_id`, the originating `message_id`, `target_session`, `decider_sessions`, `title`, `species`, `details`, and `options` (each with `option_id`, `name`, `species`). Routed requests, replies, and events SHALL bind to supervisor-authorized (runtime, adapter, target, generation) correlation. Single-target scope per the supervision scope requirement; scope SHALL NOT be inferred from generation or traffic shape.

#### Scenario: Choice request carries generation binding

- **WHEN** an adapter raises a tool-call permission mid-turn
- **THEN** the wire request includes the adapter generation id alongside the transport-native `request_id` and per-delivery correlation fields
- **AND** the adapter generation is a distinct wire field from the relay consumer generation and the identity binding epoch (never a bare comparable integer across the boundary)

#### Scenario: Response taxonomy crosses unchanged

- **WHEN** the relay resolves a choice
- **THEN** it returns `Chosen` with `option_id` and `decided_by`, or `Cancelled` with `decided_by`, `reason_code`, and optional `reason`
- **AND** the code vocabulary includes queue-full/unavailable, user-cancelled, shutdown, respawn-invalidation, and `busy_already_pending` for single-in-flight gate-drops

### Requirement: Choice authorization is relay-derived

The relay SHALL derive and check target, message, and decider authorization from its own state for every choice. Adapter-provided `decided_by`, target, deciders, and codes SHALL NOT confer authority. Reason codes SHALL be namespaced (`acp.*` vs relay-authored vocabulary) and SHALL compose without collision.

#### Scenario: Spoofed decider is rejected

- **WHEN** an adapter presents a choice response naming a decider the relay did not authorize for that target and message
- **THEN** the relay rejects the response rather than applying it

### Requirement: Choice cancellation is first-class, locally and remotely

Shutdown, respawn invalidation, or transport disconnect SHALL unblock a pending `request_choice` with a `Cancelled` return carrying the reason code — never a transport error, never an indefinite block. A disconnected pipe cannot return a wire `Cancelled` reply, so BOTH sides cancel: the adapter SHALL cancel its local resolvers on disconnect, and the relay SHALL invalidate the matching pending choice. The wire `Cancelled` is sent whenever the pipe is usable. Responses correlated to a replaced generation SHALL be discarded and SHALL NOT apply to the new generation. Human-paced choice waits SHALL have an explicit waiting policy distinct from bounded control/look RPC deadlines — the policy provides local adapter resolver cancellation and relay-side invalidation regardless of bound — and the two policies are stated separately, never one rule.

#### Scenario: Respawn invalidates pending choice

- **WHEN** a generation is replaced while its choice is pending
- **THEN** the pending call returns `Cancelled` with the respawn-invalidation code
- **AND** any late response bound to the old generation is discarded

#### Scenario: Disconnect cancels on both sides

- **WHEN** the transport drops while a choice is pending
- **THEN** the adapter cancels its local resolver without waiting for the wire
- **AND** the relay invalidates the matching pending choice rather than leaving it queued

### Requirement: Choice traffic multiplexes

A pending choice SHALL block only its own turn resolver. Cancel, fence, ack, and output traffic SHALL make progress concurrently with any pending choice. The adapter SHALL record the outcome before answering the agent (record-before-respond) and SHALL inscribe gate-drops with their code.

#### Scenario: Fence preempts pending choice

- **WHEN** a fence begins while a choice is pending
- **THEN** fence signaling and cessation observation proceed without waiting for the choice to resolve
