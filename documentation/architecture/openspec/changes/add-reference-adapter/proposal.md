## Why

The frozen agent-adapter wire contract (`add-agent-adapter-contract`, merged `4e05525d`) has never been executed: no adapter implementation exists, so the host side (Backend step 3/supervisor slice, step 4 hosting) would be built against an unvalidated contract. A deterministic adapter-side executable validates the contract first and seeds the conformance suite (the frozen contract's task 3.2, with QA Partner).

## What Changes

- Add a deterministic reference adapter executable implementing the ADAPTER SIDE of the frozen wire only: NDJSON JSON-RPC 2.0 framing, `handshakeHello` negotiation, `adapter.submit` execution + ack trichotomy, `adapter.requestChoice` raising, `relay.look` poll responses, `persistState` writes, `lifecycleEvent` emission, and typed `-32000` contract errors with correlation echo.
- Generic adapter profile with scripted turns (fixture-driven behavior, no agent child): the first executable validation stays fully deterministic — no ambient timing or randomness in any wire decision.
- Contract normativity: the frozen contract governs ALL adapter-side obligations, not only the clauses restated here; this change restates the adapter-exercised subset and adds determinism, scripting, and conformance planning. Supervisor watchdog/cessation/store-commit authority stays out of scope. ACP-profile agent-child support is an explicit follow-on, not this change.
- Determinism rule: wire decisions derive only from received frames + fixture script + supervisor-supplied deadlines + fixture-driven virtual-clock advances; clocks are injected (manual test clock), randomness is absent.
- In-tree conformance inputs under `tests/fixtures/adapter/` (scripts, frame transcripts, round-trip cases); the off-tree corpus/checker scratch (33 frames, gitignored) is consulted for coverage but not depended on.
- Conformance-suite planning joint with QA Partner (task 3.2): round-trip equivalence cases per `ideas/mcp/4` section 4 (submit-content, choice, look-poll), the must-NOT-appear list (`help`, supervision-only methods, adapter-local errors), and RG's seam-exercise rule.

## Capabilities

### New Capabilities

- `reference-adapter`: deterministic adapter-side wire implementation — framing, handshake, per-method behavior, scripted turns, determinism rule, error taxonomy.
- `adapter-conformance`: conformance inputs and round-trip equivalence cases for the reference adapter (joint with QA Partner), tracing each wire payload to its canonical tool operation per `ideas/mcp/4`.

### Modified Capabilities

- None. The frozen wire contract is untouched; no `src/relay` changes; no supervisor/hosting work (Backend owns step 3 and the supervisor slice).

## Impact

- New `src/adapter/` library module + `agentmux-adapter` binary; new `tests/fixtures/adapter/` fixtures; no changes to `src/relay`, relay behavior, CLI, MCP, or TUI.
- Reviews: Reviewer General (tier-1), then Advisor (tier-2), then operator. The frozen contract's task 3.2 (conformance suite) executes jointly with QA Partner after this change's approval; cases are planned here and executed after step 3 lands, since the canonical operation functions they round-trip against do not exist in-tree yet.
