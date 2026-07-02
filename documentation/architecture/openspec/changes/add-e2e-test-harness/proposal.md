# Change: Add e2e test harness with deterministic Coordinator simulator

## Why

Many of agentmux's e2e tests are driven by a human (or another live agent
session) playing "Coordinator" or "Operator" — examples from 0.7.0 and
0.8.0 testing include the multi-recipient envelope conformance test, the
SIGTERM-during-busy graceful-shutdown test, cross-bundle migration smoke
tests, namespacing matrix tests, and the `acp-prime-timeout-and-wedge-detection`
regression set. These tests are slow, non-deterministic, and cannot run in
CI without a human in the loop. The Coordinator (`master`) is itself an LLM
session, so automating Coordinator behavior requires a harness that can
deterministically simulate another agent's role in a scripted e2e flow.

The 0.9.0 milestone captures several recent fixes that have no regression
coverage because they require a Coordinator-or-equivalent driver:
`issues/relay/25` (multi-recipient envelope conformance, fixed at
`57043ee`), `acp-prime-timeout-and-wedge-detection` (merged
`f6c05d6`), and `add-cross-relay-sender-attribution` (merged via
`bac8d7f`). All three would benefit from a CI-runnable regression test
driven by a scripted harness.

## What Changes

- Add a `qa-harness` (working name) principal in a `test` namespace, e.g.
  `qa-harness@agentmux-test`. The harness is a **Coordinator simulator
  relay peer** (not an ACP agent client and not a bundle host) that:
  - Connects to the relay as a thin client and uses the standard
    `agentmux_send` / `agentmux_look` envelope protocol.
  - Auto-acknowledges structured pings on a known protocol
    (`Ping N` -> `Pong N`) by replying via `agentmux_send`.
  - Reads a test script from disk and drives it against a target bundle
    via the standard `send`/`look` envelope protocol.
- Add a CLI subcommand `agentmux test harness run --script <path>
  [--target-bundle <name>]` that stands up the harness principal, hosts
  the test bundle, runs the script, and exits non-zero on assertion
  failure.
- Add a v1 script grammar supporting these operations:
  - `expect <pattern> <timeout_ms>` — wait for an inbound envelope whose
    body matches the regex `pattern` (Rust `regex` crate syntax, with
    capture groups numbered by group index); fail on timeout.
  - `send <target> <body>` — send `body` to the canonical session id
    `target` via `agentmux_send`.
  - `assert_equal <expected> <actual>` — `actual` MUST be `last_expect`
    or `last_expect[N]` (Nth capture group); exit non-zero otherwise
    with `unsupported_assert_expr`.
  - `raise_signal SIGTERM <pid>` (test only) — issue SIGTERM to a
    harness-registered test process. PIDs not registered with the
    harness are rejected with `harness_signal_unauthorized`.
  - `ignore <pattern>` — opt in to silent drop of envelopes whose
    body matches `pattern` (Rust `regex` crate syntax). The only
    operation that permits a non-ping envelope to be silently dropped;
    without `ignore` (or `expect`) covering a body, the harness exits
    non-zero with `expect_drop_unmatched`.
- Add a `agentmux test bundle up <name>` and
  `agentmux test bundle down <name>` helper that hosts the test bundle
  in isolation, not auto-started by `agentmux host relay` -- the test
  bundle is opt-in per CI run.
- The harness introduces NO recipient-side cross-namespace gate. The
  existing data-driven authorization spine (requester `send` scope
  against the uniform `self`/`home`/`all` threshold) governs reach to
  `*@<test_bundle>`, exactly as for any other cross-bundle send. The
  test bundle's safety story relies on `test-isolated=true` (no relay
  autostart) plus the requester-side `send` scope model, not on a
  new handler-side exception.

## Impact

- Affected specs:
  - new `e2e-test-harness` capability (harness grammar, auto-ack
    protocol, cross-namespace reachability under existing
    authorization).
  - `MODIFIED Requirements` on `cli-surface` (new `agentmux test`
    subcommand in the unified command topology).
  - `MODIFIED Requirements` on `runtime-bootstrap`
    (`Bundle Autostart Eligibility Field` adds `test-isolated` boolean
    to per-bundle config; `Host Relay No-Selector Autostart
    Resolution` filters out `test-isolated=true` bundles from the
    autostart set; explicit `agentmux test bundle up` override path).
  - `MODIFIED Requirements` on `session-relay` (`Bundle Configuration
    Includes Autostart Eligibility` adds the `test-isolated` boolean
    to the configuration schema; the autostart-selection impact is
    owned by `runtime-bootstrap`).
  - No change to `mcp-tool-surface` -- the harness uses the existing
    `send`/`look` envelope protocol via `agentmux_send` /
    `agentmux_look`, not new MCP tools.
  - No change to `relay-routing-layer` or to the authorization
    spine in `session-relay` -- cross-namespace reach to
    `*@<test_bundle>` uses the existing data-driven model.
- Affected code: `src/commands/test/` (new), `src/commands/mod.rs`
  (router), `src/runtime/bootstrap.rs` (test-bundle opt-in
  deserialization; `serde` rename `test-isolated` -> `test_isolated`),
  `src/relay/host.rs` (autostart filter rejecting `test-isolated=true`
  bundles), and `src/test_harness/` (new; relay peer + script
  interpreter). No ACP transport code is touched in v1.
- Tests: `tests/unit/test_harness/` (script grammar),
  `tests/integration/e2e/` (full scripted flows against a real
  bundle). Initial scripted tests: `relay-25-conformance`,
  `sigterm-choreography`, `cross-relay-attribution`. None of the v1
  scripted tests require an ACP worker running inside the harness
  bundle; the harness is a pure relay peer in v1.

## Non-Goals (v1)

- The harness is NOT a general-purpose agent simulator. It is a scripted
  Coordinator-or-equivalent; the v1 script grammar is intentionally tiny.
- The harness does NOT replace the real `master` Coordinator session for
  production traffic. Production Coordinator lives in `agentmux` (or
  another production bundle) and the harness never appears in any
  production bundle's principal list.
- The harness does NOT spawn live agentmux MCP servers or ACP workers;
  it is a relay peer, not a bundle host or an ACP client. Bundle
  hosting is performed by the existing `agentmux host relay` path.
- The harness does NOT include GPT/LLM-driven simulation. v1 is fully
  scripted; future work may add an LLM-backed Coordinator mode if scripted
  flows are insufficient.
- The harness does NOT touch `src/acp/transport.rs` or any ACP code
  path in v1. v1 scripted tests are pure relay flows (envelope
  recipients, SIGTERM, `on_behalf_of` end-to-end). v2 may add ACP-touching
  flows if needed; the design allows this without changing the v1
  surface.

## Gate

This change is gated on:

1. `issues/acp/12` (opencode ACP sessions expose only read-only
   agentmux tool subset) reaching root cause, AND
2. A workaround documented in `procedures/` for scripted flows that
   depend on write-capable MCP tools, in case the root-cause fix has
   not yet shipped by the time scripted tests need to run.

The harness relies on the same MCP tool surface that ACP-launched test
agents will use; if that surface is unstable, scripted flows that depend
on `send` will fail intermittently. Without root cause and a workaround,
the v1 scripted tests cannot be relied on in CI.

A follow-up extraction (newline-framed JSON IO into `src/runtime/io.rs`,
used by both `AcpStdioClient` and the harness) is a candidate for a
separate change but is not a precondition for this proposal.
