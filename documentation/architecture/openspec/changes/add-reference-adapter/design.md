## Context

The frozen wire (`add-agent-adapter-contract`, 6 specs, 35 requirements, 54 scenarios, merged `4e05525d`) defines NDJSON JSON-RPC 2.0 over stdio with methods `adapter.handshakeHello`, `adapter.requestChoice`, `relay.look`, `adapter.submit`, `adapter.persistState`, `adapter.lifecycleEvent`, `relay.supervisionSignal`. No adapter code exists in-tree (`src/` has no adapter module); Backend owns the supervisor/hosting side (step 3 `DeliveryContext`, supervisor slice); QA Partner co-owns the conformance suite (task 3.2). The off-tree corpus/checker scratch (33 frames) informed spec review but is gitignored. Semantic cross-check: `ideas/mcp/4` (submit = authorized-send transport, requestChoice = decide, look = look).

## Goals / Non-Goals

**Goals:**

- First executable validation of the frozen contract, adapter side only.
- Bit-for-bit deterministic wire behavior for a given frame script (conformance-seedable).
- Crate/binary layout fixed in this change so implementation has a skeleton to fill.

**Non-Goals:**

- No supervisor, hosting, `DeliveryContext`, or `src/relay` changes.
- No ACP-profile agent child (scripted generic profile only; ACP follow-on later).
- No conformance-suite execution (task 3.2, joint with QA) — this change plans its inputs and cases only.
- No SDK types on the wire; protocol behavior only.

## Decisions

- **Crate layout: `src/adapter/` library + `agentmux-adapter` binary.** Library modules: `frame` (NDJSON read/write, dual inbound bounds, 64KiB bootstrap ceiling, `-32000` error shape with correlation discipline), `binding` (bootstrap establishment from hello result, then captured-binding plus direction plus wire-id pending-table admission, never inferred), `handshake` (major equality plus minor-window overlap, matched-pairs limit resolution, mandatory `latency_declarations`), `submit` (single-flight gate, scripted turn execution, ack trichotomy incl. `RefusedBusy`), `choice` (raise + block resolver, dual-cancel on disconnect, record-before-respond), `look` (snapshot replies with freshness metadata, adapter-side prime-wait), `persist` (write submission, tag-echo, duplicate-id tracking client-side), `lifecycle` (ready/degraded/failed emission, never dictating supervision outcome), `script` (fixture-script interpreter driving turns). Binary `src/bin/agentmux_adapter.rs` stays thin: argv parse, stdio loop, exit codes. Alternative (single-file binary) rejected: the conformance suite needs to drive the library without spawning for unit-level round-trip cases. New-obligation homes (no new modules; the nine-module list is complete): outbox bounds, queue caps, and the control lane live in `frame`; originated-call in-flight caps and the pending-call table live in `choice` (persist writes tracked in `persist`); ping/pong liveness lives in `lifecycle` alongside the local diagnostic/inscription sink (shed gate-drops, cleanup records); per-type cancel/cleanup terminalization lives in each method module (`submit`, `choice`, `look`, `persist`), coordinated through `binding` teardown.
- **Scripted generic profile first.** Turns follow a fixture script (TOML): on submit → emit scripted lifecycle events, optionally raise scripted `requestChoice`, ack per script. Alternative (ACP agent child now) rejected: a child process introduces nondeterminism the first validation must not have; the contract's generic profile exists exactly for this.
- **Deterministic virtual clock, no randomness.** All waits derive from supervisor-passed timeouts expiring only on fixture-driven virtual-clock advances carried as first-class trace events, with deterministic dispatch ordering; the live executable feeds the same injected clock from a monotonic source, and replay guarantees cover identical traces only. Randomness is absent from wire-decision paths (single rule, stated here; the spec's ban is this rule). Time source is a trait with a manual test implementation; no `SystemTime`/`rand` in wire-decision paths (grep-auditable). Scripted submission evidence models a fake agent effect boundary, not proof of real-agent acceptance. Alternative (real clock with tolerances) rejected: conformance transcripts must be byte-comparable.
- **In-tree fixtures as the conformance input source.** `tests/fixtures/adapter/`: scripts, request/response transcript pairs, adversarial frames (skew, oversize, unknown correlation, duplicate acks). The off-tree 33-frame corpus informs coverage; ACP points at it during review. Alternative (depending on scratch) rejected: gitignored inputs cannot gate CI.
- **Frozen contract governs all adapter-side obligations.** This change restates the adapter-exercised subset (framing, handshake, submit, choice, look, persist, lifecycle, plus the requirements below) and adds scripting, determinism, and conformance planning. Where this spec is silent, the contract text governs — the restated subset never narrows the normative contract. Supervisor-side authority (watchdog triggers, cessation observation, store commit/pointer advance, generation publication) is explicitly out of scope and is never implemented here.

- **No relay imports.** The adapter module's wire and product-boundary types use only std + serde/serde_json; fixture/script parsing may additionally use the existing `toml` dependency. The restriction that matters is product-boundary imports (no relay, MCP, transport, acp, tmux, or pty types) — opaque handles only, per the contract's no-back-edge requirement. Enforcement is review plus extension of `scripts/lint-delivery-protocol-boundary.py` to `src/adapter/` plus a dependency-direction test task. Violation is a review-blocking defect.

## Risks / Trade-offs

- [Script expressiveness vs determinism] → Mitigation: script language covers only the contract's observable behaviors (acks, choices, snapshots, events); anything richer waits for the ACP follow-on.
- [Fixture drift from frozen contract] → Mitigation: every fixture cites its requirement; contract revision (not this change) owns updates.
- [Supervisor absence limits validation depth] → Mitigation: a harness-side test double plays supervisor per the contract (pending-table admission, watchdog triggers) for round-trip cases; real supervisor interop is step-4 work.
- [ACP-profile gap] → Mitigation: documented follow-on; generic-profile validation does not claim ACP coverage.

## Migration Plan

Additive only: new module, new binary, new fixtures. No existing behavior changes; no rollback beyond revert. The binary is not wired into any supervisor or release path by this change.

## Open Questions

- Harness-side supervisor test double: throwaway test helper in `tests/` vs. a reusable crate for step-4 conformance — decide at implementation time with QA Partner input (leans reusable, since the frozen contract's 3.2 needs it).
- Whether the ACP-profile follow-on is its own OpenSpec change or a task under this one — decided: its own change, filed after this lands, so this change never blocks on agent-child semantics.
