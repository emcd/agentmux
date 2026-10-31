# Design: agent-adapter contract

## Context

The relay owns per-target delivery executors in-process today (`TransportImpl` enum, `AcpWorkerDriver` + `AcpTransport`). The v0.11.0 theme moves each transport behind a versioned executable agent-adapter wire boundary; the relay becomes a supervisor of adapter processes. The merged `embeddable-runtime-api` (7876828d) fixes the in-process side: no host-supplied in-process delivery implementations, observation-only boundary, normative submission-evidence trichotomy. This design specifies the wire both sides speak. Requirements live in `ideas/acp/3` (R1.1–R6.5, both tiers approved) and the `adapter-*` specs; the ACP wire-shape draft is `ideas/acp/4`.

Stakeholders: ACP lane (adapter shapes), Backend lane (supervisor side + config schema), Coordinator (sequencing), QA Partner (conformance suite, follow-on).

## Goals / Non-Goals

Goals: concrete wire operations for choice resolution, snapshot reads, and submit acknowledgment; generation-bound routing everywhere; a lifecycle handshake with neutral signals; version/skew policy decided before any interoperable adapter is built.

Non-goals: the reference adapter implementation and conformance suite (follow-on slice); changes to in-process transport behavior; streaming look (schema stays compatible, build later); SDK types on the wire (protocol behavior only, `todos/acp/29` deferred).

## Decisions

### D1: One multiplexed stdio JSON-RPC transport, both directions

Requests, responses, and notifications multiplex over a single stdio JSON-RPC channel per adapter instance — newline-delimited UTF-8 JSON, one object per frame. The `adapter.*` / `relay.*` prefix names the domain that defines the method, not the call direction: the adapter calls `adapter.requestChoice` (reverse-RPC raising a permission need), `adapter.handshakeHello` (bootstrap introduction), and `adapter.persistState` (resume writes), and emits `adapter.lifecycleEvent` notifications; the supervisor calls `adapter.submit` (delivering an authorized batch for the adapter to execute and acknowledge) and `relay.look` (polling the adapter's output). The canonical JSON-RPC mapping — methods, params, id rules, error shape — is `adapter-wire-schema`; D1's framing promise is satisfied by that mapping.

Alternative considered: a separate socket per direction. Rejected: doubles the supervision surface (two livenesses, two generations to bind) and abandons the proven stdio framing for no gain.

Alternative considered: relay-poll model for choices (adapter exposes pending-choice queue, relay polls). Rejected: it reintroduces the split-blocking-call hazard the `TransportEvent` removal (`ideas/transports/2` amendment) already settled — a choice is one blocking call, not two async halves.

### D2: Generation on every routed message; no epoch field

Every choice request/response, look request/reply, submit, and ack carries the adapter generation id. `request_id` alone never routes: ACP request ids repeat across generations (Tier-2 C4). The relay binds authorization, correlation, and stale-reply invalidation to supervisor-authorized (runtime, adapter, target, generation) correlation — uniqueness is never inferred from generation alone. Scope is single-target per the supervision scope requirement.

Three generation-like axes exist and MUST stay distinct wire fields with distinct types — never a bare comparable u64 across the boundary: the adapter generation (this contract's routing axis), the relay delivery-consumer generation (`ConsumerGenerationId`, five-step fence ack), and the identity binding epoch (merged `runtime-api`). Stale and duplicate controls — for choices and events alike — key on the adapter's own generation and MUST NOT affect replacements; relay fence-ack stays the cessation mechanism.

`epoch` is REMOVED from all routed messages. Rationale: no concrete restart/rebind trace distinguishes a fourth counter — generations are supervisor-issued and monotonic per (instance, target), echoed by the adapter and never restarted at 1, so late traffic is stale by generation inequality with no durable counters anywhere; supervisor restarts are covered by instance identity; credential lifecycle is covered by the binding epoch. The field name stays reserved for a future protocol version with this rationale recorded. The three axes stay separate wire fields.

### D3: Poll look now, streaming later — with multiplexing as the condition

`look` is a per-call relay→adapter RPC. Accepted cost: no concurrent read without touching the worker. The condition that makes the cut sound (Tier-2 C5): dispatch multiplexes, so a prime-wait in progress never blocks cancel/fence/ack/choice traffic; replies carry generation with stale/discontinuity metadata; the schema keeps `offset` + entry counts so subscribe/stream attaches additively.

Alternative considered: specifying streaming upfront. Rejected: new protocol surface that should ride the reference-adapter slice, where real usage informs the cursor/channel shape.

### D4: Policy values cross; policy lives where readiness lives

Prime-timeout, single-flight gates, and the submit boundary are enforced adapter-side; the relay passes only values (timeout durations, window sizes). This mirrors the in-process split exactly (`OutputView` owns the prime-wait; the relay supplies `prime_timeout`), so the contract is a faithful generalization rather than a redesign.

### D5: Namespaced reason codes, two authorities

Relay-authored and ACP-authored codes compose by namespace prefix (`acp.*` vs relay vocabulary) and never collide. Namespaces prevent collisions and dependency back-edges — not cross-authority spoofing, which is handled by relay-derived authorization (D6).

### D6: Relay derives authorization; adapter fields never confer it

For every choice and event, the relay checks target, message, and decider authorization from its own state. Adapter-provided `decided_by`/target/deciders/codes are claims, never proof. The supervisor likewise authorizes the target↔adapter↔generation binding; the adapter never mints principal authority (Tier-2 C1).

### D7: Neutral supervisor signals; ACP lifecycle stays inside the adapter

The generic supervisor never speaks ACP `initialize`/`new`/`load`. The adapter runs the ordered lifecycle internally and reports neutral ready/failure signals with typed codes. Current ACP timeouts and cadences (30s bootstrap bound, ETXTBSY retries, poll slices) are ACP defaults informing the timeout schema — not frozen universal wire constants (Tier-2 C6).

## Risks / Trade-offs

- [Risk] Wedged or dead adapter ignores terminate/observe RPCs → Mitigation: supervisor-owned out-of-band termination and cessation observation of the whole effect-capable set (descendants/FD holders); unreachable ≠ ceased; fail-stop on negative fence (Backend slice, Tier-2 C3).
- [Risk] Completion races the submit ack; late/duplicate acks → Mitigation: declare-before-effect, generation-scoped request ids, single terminal resolution per correlation, no blind replay (spec `adapter-submit-acks`).
- [Risk] Superseded generation overwrites a newer snapshot or choice → Mitigation: generation on all replies; stale-reply invalidation on replacement (specs `adapter-choice-rpc`, `adapter-look-poll`).
- [Risk] Pending choice head-of-line blocks fence/cancel → Mitigation: multiplexing is a contract requirement, not an implementation hint — but logical multiplexing alone does not survive stdio backpressure, so frames and queues are bounded and out-of-band fencing plus disconnect cancellation stay independent of a blocked writer (Tier-2 item 4).
- [Risk] Human-paced choice waits vs bounded control RPCs need different policies → Mitigation: human-paced waits get an explicit waiting policy distinct from bounded control/look deadlines (no unbounded mandate — the policy itself is the requirement), always with local adapter resolver cancellation and relay-side invalidation. The two policies are stated separately, never one rule (Tier-2 item 4).
- [Risk] Independently released adapters skew from the relay → Mitigation: version/framing/limits negotiation is settled before the reference adapter; skew policy gates that slice (Q2).
- [Trade-off] Poll look loses free concurrent reads. Bounded and accepted: the in-process handle already serializes, and streaming attaches later without schema breakage.

## Migration Plan

None required: purely additive contract, no implementation in this change. The reference-adapter slice migrates nothing; in-process transports keep working until an adapter replaces them per target.

## Proposed freeze resolutions (under review — proposals, not decisions)

The following are ACP+Backend proposals for tier review. None is decided until the freeze; SHALL-language below is draft-contract voice, not normative approval.

1. Version/framing/limits: major equality required, minor overlap of declared windows, limits resolved down — per the supervision version-skew requirement. Skew policy gates the reference adapter as stated.
2. Stale-write fence: cessation-before-replacement with supervisor-exclusive serialized commit authority, observed out-of-band cessation, immutable per-generation namespaces, fresh-instance restart currency — per the supervision cessation requirement.
3. Custody: canonical identity, current-generation currency, and the resume store bytes at the supervisor (never parsing blobs); adapter submits via persist_state, hydrates in-handshake; within-instance import at publish, no cross-restart import.
4. Supervision model: relay-spawned only; no self-registration, no orphan adoption (terminate + relaunch on restart), no detach in this revision.
5. Human-choice wait policy: unbounded-but-invalidatable with an operator-configured ceiling, dual-cancel path shared with disconnect — per the supervision wait-ceiling requirement.
6. Backpressure shape: negotiated handshake limits resolved down, refuse-with-typed-code frozen; bound values negotiated, not hardcoded.
7. Scope: single-target per adapter instance — per the supervision scope requirement.
8. Epoch: REMOVED from all routed messages — supervisor-issued monotonic generations, field name reserved with rationale per D2.

ACP follow-up obligation: none remaining on latency publication — `latency_declarations` are handshake fields per the watchdog requirement (todos/acp/32 tracks the adapter timeout-schema side).
