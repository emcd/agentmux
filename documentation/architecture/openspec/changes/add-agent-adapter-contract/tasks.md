## 1. Contract freeze (completion gate — all required before approval)

- [ ] 1.1 Tier-1 re-review of the Tier-2 rework — Reviewer General.
- [ ] 1.2 Tier-2 sign-off — Advisor.
- [ ] 1.3 Backend completes the supervision interface slice (outer look bound, fence mechanism, version/skew policy, submit payloads, event schema, ownership model).
- [ ] 1.4 Joint freeze deliverables (ACP + Backend, ALL required before approval): `epoch` removed from all routed messages with rationale recorded (per D2); single-target scope decided; supervisor-owned resume store with persist/hydrate interface and explicit import decided; version/framing/limits negotiated; packing payload schema with membership validation; event schema; bounded per-attempt submission resolution when the adapter stays connected but withholds ack (watchdog policy with declaration fields); normative backpressure bounds with control cap; chosen human-choice wait policy. Unknown/no-ack text is always read with its bounded-trigger requirement, never as immediate timeout-less classification.

Approval as a frozen wire contract requires 1.1–1.4 ALL complete. Backend *implementation* against the frozen contract is deferred work, not a freeze gate — but the interface slice (1.3) and the joint freeze (1.4) are prerequisites for calling this a frozen contract.

## 2. Backend implementation (deferred, not a freeze gate)

- [ ] 2.1 Backend implements the supervisor slice against the frozen contract. Owned by the Backend lane; implementation follows the freeze, it does not gate it.

## 3. Deferred follow-on (explicitly NOT part of this change's completion)

- [ ] 3.1 Deterministic reference adapter implementing the frozen wire.
- [ ] 3.2 Conformance suite with QA Partner (each spec scenario is a test case, including negative auth cases and disconnect-after-write cases).
- [ ] 3.3 Streaming look attached additively to the frozen poll schema.

## 4. Explicit non-tasks

- [ ] 4.1 No in-process transport behavior changes in this change.
- [ ] 4.2 No SDK adoption (`todos/acp/29` stays deferred behind the wire boundary).
