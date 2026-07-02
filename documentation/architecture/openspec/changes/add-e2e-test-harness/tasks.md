## 0. Gate Check (must complete before any task in section 1 or later)

The proposal is gated on TWO conditions that must hold before
implementation begins. This section MUST be checked off in full
before any task in section 1, 2, 3, 4, 5, or 6 is started. The
gate lives in `proposal.md` prose AND here as a tracking task;
both must agree.

- [ ] 0.1 Confirm `issues/acp/12` (Opencode ACP sessions expose only
      read-only agentmux tool subset) is **root-caused** with a
      resolution recorded in the issue body. The original June 27
      investigation recorded the issue as "owner: ACP Specialist to
      run to ground in 0.9.0 cycle"; that resolution must be in
      place. If not root-caused, halt and route back to ACP
      Specialist. Do NOT proceed to section 1.
- [ ] 0.2 Confirm a workaround procedure exists under `procedures/`
      covering scripted harness flows that depend on write-capable
      MCP tools (i.e., `agentmux_send` / `agentmux_raww` /
      `agentmux_choose`). The procedure MUST document how a CI
      runner or developer runs the v1 scripted tests when the
      ACP-side root-cause fix has not yet shipped. Suggested filename:
      `procedures/<filename>.md` (propose a name when the procedure
      lands; place under `procedures/` per the project's
      `procedures/`-folder convention).
- [ ] 0.3 Confirm both `issues/acp/12` and the new `procedures/`
      entry are referenced by a tracking note in `coordination/mcp/`
      or `coordination/qa/` confirming the gate cleared. This makes
      the gate exit criterion machine-discoverable from the
      coordination lane notes without re-reading the proposal.

If any of 0.1, 0.2, or 0.3 is unchecked, **do not start any task in
sections 1-6**. Open a follow-up dispatch to the appropriate lane
(ACP Specialist for 0.1, Operator for 0.2, Coordinator for 0.3) and
hold.

## 1. CLI subcommand

- [ ] 1.1 Add `src/commands/test/mod.rs` with `test` subcommand router
- [ ] 1.2 Add `src/commands/test/harness.rs` with `harness run --script <path>`
      argument parsing and CLI surface per `cli-surface/spec.md` patterns
- [ ] 1.3 Add `src/commands/test/bundle.rs` with `test bundle up/down`
      commands for opt-in test bundle hosting
- [ ] 1.4 Register `test` in `src/commands/mod.rs` router and `help`
- [ ] 1.5 Add `tests/unit/commands/test_harness.rs` (argument validation)
- [ ] 1.6 Add `tests/integration/cli/test_harness.rs` (CLI surface)

## 2. Test bundle opt-in hosting

- [ ] 2.1 Add a per-bundle `test_isolated: bool` (default false) to
      `BundleConfiguration`; the TOML key is `test-isolated` (kebab-case)
      and the Rust field is `test_isolated: bool` with a
      `#[serde(rename = "test-isolated")]` mapping. Reject
      `test-isolated=true` bundles in `agentmux host relay` autostart path.
- [ ] 2.2 Add a `agentmux test bundle up <name>` path that hosts the
      bundle regardless of the autostart gate, but only when the bundle
      has `test-isolated=true` in its configuration.
- [ ] 2.3 (No new policy flag) Cross-namespace reachability to
      `*@<test_bundle>` is governed by the existing data-driven
      authorization spine. No `policies.relay.allow_cross_namespace_to_test`
      flag, no recipient-side gate, no handler-side exception.

## 3. Harness relay peer (and possibly CLI-subprocess implementation)

- [ ] 3.1 Open Question: in-process peer vs. CLI subprocess. Default
      path: in-process peer in `src/test_harness/relay_peer.rs`,
      using the agentmux envelope protocol directly. Fallback if
      latency is unacceptable: shell out to `agentmux send` /
      `agentmux look` from inside the script interpreter and skip
      `relay_peer.rs`. Decision recorded in `design.md` open
      questions before implementation starts.
- [ ] 3.2 Add `src/test_harness/mod.rs` (public surface)
- [ ] 3.3 Add `src/test_harness/relay_peer.rs` (thin client using
      `agentmux_send` / `agentmux_look`; no ACP, no MCP server, no
      bundle hosting)
- [ ] 3.4 Add `src/test_harness/script.rs` (script parser/grammar
      v1: `expect`, `send`, `assert_equal`, `raise_signal`, `ignore`;
      regex uses Rust `regex` crate; `assert_equal` only accepts
      `last_expect` / `last_expect[N]`; `ignore` opts in to silent
      drop for matching bodies)
- [ ] 3.5 Add `src/test_harness/runner.rs` (script interpreter;
      async runtime; timeout handling; `expect_drop_unmatched`
      failure mode for non-ping envelopes with no script expect
      within 250ms and no matching `ignore`)
- [ ] 3.6 Add `src/test_harness/protocol.rs` (`Ping N` -> `Pong N`
      auto-ack on the harness side via `agentmux_send`; rejection
      of unsupported `assert_equal` expressions)
- [ ] 3.7 (Follow-up extraction, NOT a precondition) extract
      newline-framed JSON IO into `src/runtime/io.rs` for shared
      use by `AcpStdioClient` and the harness. Captured under
      a separate change.
- [ ] 3.8 Add `tests/unit/test_harness/script.rs` (grammar;
      capture-group parsing; `unsupported_assert_expr` rejection;
      `ignore` body matching)
- [ ] 3.9 Add `tests/unit/test_harness/runner.rs` (timeout
      handling; assertion failure; `expect_drop_unmatched` for
      unmatched bodies)

## 4. Initial scripted tests (v1)

- [ ] 4.1 `relay-25-conformance`: send to N=2 targets across bundles;
      assert each delivered envelope contains references to all N
      recipients in `To`/`Cc` (regression for `issues/relay/25` fix at
      `57043ee`)
- [ ] 4.2 `sigterm-choreography`: register harness as target; issue N
      pings; raise SIGTERM mid-stream; assert relay exits within
      ~5s grace period rather than hanging (regression for
      `todos/relay/17`, `relay/79`, `relay/80`, and
      `acp-prime-timeout-and-wedge-detection`)
- [ ] 4.3 `cross-relay-attribution`: send cross-bundle; assert
      `on_behalf_of` is preserved end-to-end through delivered envelope
      and `Send` response (regression for `add-cross-relay-sender-attribution`
      at `bac8d7f`)
- [ ] 4.4 Add `tests/integration/e2e/relay_25_conformance.rs`
- [ ] 4.5 Add `tests/integration/e2e/sigterm_choreography.rs`
- [ ] 4.6 Add `tests/integration/e2e/cross_relay_attribution.rs`

## 5. Specs and validation

- [ ] 5.1 Write `specs/e2e-test-harness/spec.md` (new capability)
- [ ] 5.2 `MODIFIED Requirements` on `specs/cli-surface/spec.md`
      (Unified Agentmux Command Topology: add `test` subcommand)
- [ ] 5.3 `MODIFIED Requirements` on `specs/runtime-bootstrap/spec.md`
      (Bundle Autostart Eligibility Field: add `test-isolated`
      boolean; Host Relay No-Selector Autostart Resolution: filter
      out `test-isolated=true` bundles; explicit `agentmux test
      bundle up` override path)
- [ ] 5.4 `MODIFIED Requirements` on `specs/session-relay/spec.md`
      (Bundle Configuration Includes Autostart Eligibility: add
      `test-isolated` boolean to the configuration schema; the
      autostart-selection impact is owned by `runtime-bootstrap`)
- [ ] 5.5 `openspec validate add-e2e-test-harness --strict`
- [ ] 5.6 `openspec validate --all --strict`

## 6. CI integration (out of scope for v1, but documented)

- [ ] 6.1 (follow-up) Add a CI workflow that runs the v1 scripted tests
      on every PR; gate the merge on green
- [ ] 6.2 (follow-up) Add per-bundle CI matrix entries for the e2e
      flows that need cross-bundle routing
