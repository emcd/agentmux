# Design: agent-adapter contract

## Context

The relay owns per-target delivery executors in-process today (`TransportImpl` enum, `AcpWorkerDriver` + `AcpTransport`). The v0.11.0 theme moves each transport behind a versioned executable agent-adapter wire boundary; the relay becomes a supervisor of adapter processes. The merged `embeddable-runtime-api` (7876828d) fixes the in-process side: no host-supplied in-process delivery implementations, observation-only boundary, normative submission-evidence trichotomy. This design specifies the wire both sides speak. Requirements live in `ideas/acp/3` (R1.1–R6.5, both tiers approved) and the `adapter-*` specs; the ACP wire-shape draft is `ideas/acp/4`.

Stakeholders: ACP lane (adapter shapes), Backend lane (supervisor side + config schema), Coordinator (sequencing), QA Partner (conformance suite, follow-on).

## Goals / Non-Goals

Goals: concrete wire operations for choice resolution, snapshot reads, and submit acknowledgment; generation-bound routing everywhere; a lifecycle handshake with neutral signals; version/skew policy decided before any interoperable adapter is built.

Non-goals: the reference adapter implementation and conformance suite (follow-on slice); changes to in-process transport behavior; streaming look (schema stays compatible, build later); SDK types on the wire (protocol behavior only, `todos/acp/29` deferred).

## Decisions

### D1: One multiplexed stdio JSON-RPC transport, both directions

Adapter→relay reverse calls (`request_choice`, lifecycle events) and relay→adapter calls (`look`, submit, fence signals) multiplex over a single stdio JSON-RPC channel per adapter instance — the same framing the ACP client already speaks (`client/protocol.rs`, `client/reader.rs`).

Alternative considered: a separate socket per direction. Rejected: doubles the supervision surface (two livenesses, two generations to bind) and abandons the proven stdio framing for no gain.

Alternative considered: relay-poll model for choices (adapter exposes pending-choice queue, relay polls). Rejected: it reintroduces the split-blocking-call hazard the `TransportEvent` removal (`ideas/transports/2` amendment) already settled — a choice is one blocking call, not two async halves.

### D2: Generation on every routed message; epoch defined at freeze

Every choice request/response, look request/reply, submit, and ack carries the adapter generation id. `request_id` alone never routes: ACP request ids repeat across generations (Tier-2 C4). The relay binds authorization, correlation, and stale-reply invalidation to supervisor-authorized (runtime, adapter, target, generation) correlation — uniqueness is never inferred from generation alone. Scope is single-target per the supervision scope requirement.

Three generation-like axes exist and MUST stay distinct wire fields with distinct types — never a bare comparable u64 across the boundary: the adapter generation (this contract's routing axis), the relay delivery-consumer generation (`ConsumerGenerationId`, five-step fence ack), and the identity binding epoch (merged `runtime-api`). Stale and duplicate controls — for choices and events alike — key on the adapter's own generation and MUST NOT affect replacements; relay fence-ack stays the cessation mechanism.

The `epoch` field is reserved in every routed message and carries NO provisional implementation meaning: until the joint freeze defines it explicitly, implementations MUST NOT read it. Treating an undefined field as adapter-scoped would bake divergent meanings into both sides before the definition exists.

### D3: Poll look now, streaming later — with multiplexing as the condition

`look` is a per-call relay→adapter RPC. Accepted cost: no concurrent read without touching the worker. The condition that makes the cut sound (Tier-2 C5): dispatch multiplexes, so a prime-wait in progress never blocks cancel/fence/ack/choice traffic; replies carry generation with stale/discontinuity metadata; the schema keeps `offset` + entry counts so subscribe/stream attaches additively. (`epoch` is reserved everywhere per D2, including here.)

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

## Decided at freeze (were Open Questions 1-4, plus wait policy, backpressure shape, and scope)1. Version/framing/limits: major equality required, minor overlap of declared windows, limits resolved down — per the supervision version-skew requirement. Skew policy gates the reference adapter as stated.
2. Stale-write fence: generation-tagged replacement picked (tags checked against relay-held currency on every write); CAS and advisory lock rejected.
3. Custody: currency (identity + current generation) at the relay, opaque bytes at the adapter, fenced by tag.
4. Supervision model: relay-spawned only; no self-registration, no orphan adoption (terminate + relaunch on restart), no detach in this revision.
5. Human-choice wait policy: unbounded-but-invalidatable with an operator-configured ceiling, dual-cancel path shared with disconnect — per the supervision wait-ceiling requirement.
6. Backpressure shape: negotiated handshake limits resolved down, refuse-with-typed-code frozen; bound values negotiated, not hardcoded.
7. Scope: single-target per adapter instance — per the supervision scope requirement.

ACP follow-up obligation: the adapter timeout schema must publish max ack latency per content type, or the watchdog-deadline-vs-latency ordering in the watchdog requirement is unenforceable.

## Remaining for freeze: `epoch` definition (reserved, no provisional meaning until defined).
