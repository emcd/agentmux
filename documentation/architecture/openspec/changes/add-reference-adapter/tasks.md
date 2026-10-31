## 1. Crate skeleton

- [x] 1.1 Add `src/adapter/` module tree (`frame`, `binding`, `handshake`, `submit`, `choice`, `look`, `persist`, `lifecycle`, `script`) with crate-level exports carrying no relay, MCP, or transport types.
- [x] 1.2 Add `agentmux-adapter` `[[bin]]` (`src/bin/agentmux_adapter.rs`): argv parse, stdio loop, exit codes; binary stays thin, all logic in the library.
- [x] 1.3 Add injected clock trait + manual test clock; grep-verify no `SystemTime`/`rand` in wire-decision paths.
- [x] 1.4 Extend `scripts/lint-delivery-protocol-boundary.py` to cover `src/adapter/` (forbid relay/acp/tmux/pty/transports imports) as the no-back-edge enforcement.

## 2. Wire implementation

- [x] 2.1 Implement NDJSON framing with an accumulated-frame-bytes inbound bound (no length prefix on the wire; refuse past the bound without body-sized allocation) and negotiated `max_frame_bytes` enforcement, plus `-32000` error construction with correlation echo.
- [x] 2.2 Implement handshake: major equality plus minor-window overlap, matched-pairs limit resolution, mandatory `latency_declarations`, typed skew/validation refusal.
- [x] 2.3 Implement submit path: single-flight gate with `RefusedBusy`, scripted turn execution, outcome literals `Submitted`/`RefusedBusy`/`TransportUnavailable`/`SubmissionUnknown` with `evidence_basis` (trichotomy: submission / non-submission / unknown).
- [x] 2.4 Implement choice raising: handshake included in originated-call discipline, generation-bound requests, originator-monotonic wire ids from 1 with pending-table admission, resolver blocking limited to own turn, disconnect dual-cancel, record-before-respond.
- [x] 2.5 Implement look polls: freshness metadata, adapter-side prime-wait from relay timeout, stale-tagged results, superseded-generation handling.
- [x] 2.6 Implement persist writes (tagged, no currency checks) and lifecycle event emission with namespaced codes.
- [x] 2.7 Implement adapter-side contract obligations: TX/RX queue bounds with perspective rule and bounded drain-wait, control priority lane, event coalescing classes with last-authoritative retention, originated-call in-flight caps, ping/pong liveness, and cancel/cleanup terminalization per type.

## 3. Scripted turns and fixtures

- [x] 3.1 Implement the TOML fixture-script interpreter driving scripted turns (acks, choices, snapshots, events).
- [x] 3.2 Add `tests/fixtures/adapter/` inputs: scripts, transcript pairs, adversarial frames, and bounded protocol fixtures (queue-cap, in-flight cap, ping/pong, coalescing, cleanup, correlation_invalid, unparseable); every fixture cites its contract requirement by name.
- [x] 3.3 Prove byte-identical transcript replay for a fixed script (determinism gate).

## 4. Conformance planning (joint with QA Partner; input to the frozen contract's task 3.2)

- [x] 4.1 Draft round-trip equivalence cases per `ideas/mcp/4` (submit-content, choice, look-poll) with adapted-is-direct assertions.
- [ ] 4.2 Apply the exclusion list (`help`, supervision-only methods, adapter-local errors) and the seam-exercise rule to every new case.
- [ ] 4.3 Run `cargo fmt`, clippy, and the canonical `cargo nextest run` suite; `openspec validate --strict`.
