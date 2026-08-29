# Design: e2e test harness with deterministic Coordinator simulator

## Context

The agentmux project has accumulated several critical fixes since 0.7.0
that have no CI-runnable regression coverage because the test setup
requires a Coordinator-or-equivalent driver that is itself an LLM session.
Recent examples (per `coordination/general/16` v0.9.0 milestone scope):

- `issues/relay/25` (multi-recipient envelope conformance) — fixed at
  `57043ee`. Reproduced manually on 2026-06-10; no automated regression.
- `acp-prime-timeout-and-wedge-detection` — merged `f6c05d6`. Lock-in
  test would be a SIGTERM-during-busy dance.
- `add-cross-relay-sender-attribution` — merged via `bac8d7f`. Lock-in
  test would assert `on_behalf_of` end-to-end.

The Coordinator is `master@agentmux` (a Claude Sonnet 4.6 LLM session
per the team's working agreement). To run a test of the form
"Coordinator sends X; agent responds; assert Y" in CI, we need
something that can deterministically play the Coordinator role without
being an LLM.

## Goals

- A v1 harness that is fully scripted (no LLM), CI-runnable, and
  sufficient to drive the three regression tests above.
- Strong safety: the harness MUST NOT be reachable from production
  traffic under default conditions. It lives in a `test` namespace;
  cross-namespace reach is governed by the existing data-driven
  authorization spine (requester `send` scope against the uniform
  `self`/`home`/`all` threshold), with no new handler-side exception
  for the test-bundle case. Production traffic without `send` scope
  `all` cannot reach `*@<test_bundle>`; the CI test runner's
  `send=all` is the only legitimate cross-bundle sender.
- Minimal surface area: the harness is a thin relay peer. No new MCP
  tools. No new bundle hosting primitive (reuses `agentmux host relay`).
  No ACP transport code paths are touched in v1.
- Small script grammar: just enough to express the three v1 tests, plus
  a few obvious extensions (`assert_equal`, `raise_signal`).

## Non-Goals (v1)

- LLM-backed simulation. v1 is fully scripted. Future work may add an
  LLM-driven mode if scripted flows turn out to be insufficient.
- Replacing the real `master` Coordinator for production traffic. The
  harness is test-only.
- New MCP tools. The harness uses the existing `send`/`look` envelope
  protocol via `agentmux_send` / `agentmux_look`.
- New bundle hosting primitive. The harness is hosted by the existing
  `agentmux host relay` path with a per-bundle `test-isolated=true`
  opt-in (TOML key; Rust field `test_isolated: bool`).
- Pty transport coverage. Out of scope until Pty bootstrap lands
  (`todos/pty/3`, `todos/pty/4`).
- ACP code paths. v1 does not touch `src/acp/transport/` or any
  ACP code. v1 scripted tests are pure relay flows; v2 may add
  ACP-touching flows.

## Decisions

### Decision 1: Thin relay peer, not a full agentmux session and not an ACP client

A full agentmux session requires bundle loading, relay connection,
MCP server hosting, and ACP agent child spawning. None of that is
needed for the harness's job. A thin relay peer just speaks the
agentmux `send`/`look` envelope protocol over the relay wire, which
is enough to drive the harness's scripted flows. The harness is
explicitly NOT an ACP client: it does not speak ACP JSON-RPC over
stdio, does not own an ACP agent Child, and does not drive
`session/prompt` or any other ACP method. Its IO surface is the
relay envelope protocol, full stop.

**Alternatives considered**:
- *Full agentmux session*: more machinery, but no testable
  difference in behavior. Rejected.
- *ACP client (reuse `src/acp/client.rs::AcpStdioClient`)*: ACP
  client is the orchestrator side of ACP JSON-RPC, with a child
  Child, replay buffer, ActivePrompt contract, and SharedPendingToolCalls.
  None of those responsibilities match the harness. Reuse would
  require extracting the newline-framed JSON IO primitive into
  `src/runtime/io.rs` first; that is a follow-up extraction, not
  a precondition.
- *Pure CLI subprocess invocations* (`agentmux send ...` /
  `agentmux look ...` from inside the script interpreter): the
  simplest implementation; trades latency and process-management
  complexity for a smaller code surface. **Worth pursuing as a v1
  implementation if it can express the three v1 scripted tests
  without a process-per-send latency cliff.** Open question.
- *Mock/stub relay*: would not exercise the real relay code path.
  Rejected.

### Decision 2: Shadow bundle with `test-isolated=true`, not in-band

The harness lives in its own bundle (`agentmux-test` or similar) that
is marked `test-isolated=true` (TOML key, kebab-case; the Rust
struct field is `BundleConfiguration::test_isolated: bool`). The
`agentmux host relay` autostart path REJECTS bundles with this flag
set, so the test bundle is never auto-hosted. A separate
`agentmux test bundle up <name>` command hosts it explicitly, only
for the duration of the test run.

The `test-isolated` flag has three intended values:

1. **Intent in config**: it documents in the bundle's TOML that
   this bundle is a test-harness target, not a regular interactive
   bundle. Without the flag, a future operator looking at the
   `agentmux-test` bundle's TOML has no in-band signal that it is
   exclusively a test target.

2. **Forward-looking safety story**: in a multi-tenant or
   production deployment, the test bundle MUST NOT be auto-hosted
   by the same relay that hosts interactive sessions. The flag
   makes that policy enforceable at autostart time, not just at
   `agentmux test bundle up` time.

3. **Override path for the harness**: the harness's
   `agentmux test bundle up <name>` command requires the target
   bundle to be `test-isolated=true` (rejects with a clear error
   otherwise). Without the flag, there is no in-band way to mark
   a bundle as "this is the harness's target."

In the R&D case where all bundles are worktree-shared and no bundle
is "production" in the multi-tenant sense, value (2) is mostly
belt-and-suspenders. Values (1) and (3) still apply: the operator
reading the TOML knows the bundle is a test target, and the
harness's explicit-host path has a contract to enforce.

**Alternatives considered**:
- *In-band*: harness lives in the production bundle. Hard to
  distinguish from real Coordinator; safety story is harder.
  Rejected.
- *In-process mock*: bypasses the real relay. Loses the
  regression-test value. Rejected.

### Decision 3: Per-bundle `test-isolated` flag, not a global config flag

A per-bundle flag (`test-isolated` in TOML; the Rust struct field
is `BundleConfiguration::test_isolated: bool`) keeps the safety
boundary tight: only bundles explicitly marked as test-isolated
are eligible for `test bundle up`. The `agentmux host relay` path
filters them out at autostart time.

**Alternatives considered**:
- *Global config flag* (`relay.test-isolated = true`): would
  inadvertently allow the test bundle to be hosted by the
  production relay. Rejected for safety.
- *Bundle name pattern* (`bundles named `*-test` are isolated`):
  couples to naming; could be violated by typo. Rejected in favor
  of an explicit per-bundle flag.

### Decision 4: Cross-namespace reachability uses the existing data-driven spine

Cross-namespace reach to `*@<test_bundle>` is governed by the
existing data-driven authorization spine
(`specs/relay-routing-layer/spec.md` and
`specs/session-relay/spec.md`): the requester's configured `send`
scope against the uniform `self`/`home`/`all` threshold is the
sole authority. The harness introduces NO recipient-side gate, NO
per-operation cross-namespace exception, and NO new policy flag for
the test-bundle case.

The test bundle's safety story relies on three pre-existing
properties of the runtime, not on a new gate:

- `test-isolated=true` keeps `agentmux host relay` autostart from
  hosting the test bundle, so production relays do not inadvertently
  run it. This is enforced at bundle-load time
  (`BundleConfiguration::test_isolated` on the deserialized TOML)
  and at autostart time (a `test-isolated=true` bundle is filtered
  out of the autostart set).
- A CI test runner's session is granted `send` scope `all` by the
  CI operator's relay policy, exactly as a Coordinator session is.
  Production senders without `send` scope `all` cannot reach
  `*@<test_bundle>` from a different namespace; that is the
  requester-side policy, not a test-specific gate.
- The harness's script interpreter is the only thing that listens
  on the harness principal; envelopes not matching an `expect` or
  an `ignore` operation are surfaced as `expect_drop_unmatched`
  rather than silently dropped (see Decision 6 and the
  `e2e-test-harness` spec's Harness Auto-Ack Protocol).

**Alternatives considered**:
- *Recipient-side `policies.relay.allow_cross_namespace_to_test`
  flag*: rejected. Adds a per-operation cross-namespace policy in
  handler/routing code, which violates the existing routing-layer
  invariant: "the relay SHALL NOT apply per-operation cross-namespace
  policy in handler or routing code; this data-driven spine —
  uniform tier classification plus the schema allowed-scope set —
  SHALL be the single authority for cross-namespace reach"
  (`specs/relay-routing-layer/spec.md`).
- *Production-bundle opt-in*: gives production operators a vote
  in a flag they should not own. Rejected.
- *Per-bundle `accept_from: [...]` policy*: more flexible but
  more complex; deferred to v2 if needed.

### Decision 5: Script grammar is small and v1-specific

v1 grammar: `expect <pattern> <timeout_ms>`, `send <target> <body>`,
`assert_equal <expected> <actual>`, `raise_signal SIGTERM <pid>`,
`ignore <pattern>`.
`actual` MUST be `last_expect` or `last_expect[N]` (Nth capture
group from the most recent `expect` regex match). Other
expressions are rejected with `unsupported_assert_expr`.

`expect <pattern>` and `ignore <pattern>` use the Rust `regex` crate
syntax. Patterns are anchored by default (a `^` implicit on the
start of body matching). Capture groups are numbered 1-based by
`(... )`.

`ignore <pattern>` is the only operation that permits a non-ping
envelope to be silently dropped. Without `ignore` (or `expect`)
covering a body, the harness exits non-zero with
`expect_drop_unmatched`. This makes silent fallbacks explicit:
scripts that want to permit chatty background traffic opt in
deliberately.

Future expansion is allowed (additional operations can be added
without changing existing scripts) but the v1 grammar is locked
to these five operations. Each operation maps to one or two
`agentmux_send` / `agentmux_look` calls.

**Alternatives considered**:
- *Full scripting language (Python, etc.)*: heavyweight dependency;
  test scripts would not be reviewable as part of the agentmux
  repo. Rejected.
- *YAML/JSON config*: less expressive than a small grammar. Rejected.
- *Tcl-style*: not familiar to the team. Rejected.
- *PCRE instead of Rust regex*: Rust regex is in the standard
  toolchain; PCRE would be a new dependency. Rejected.

### Decision 6: No silent fallbacks on auto-ack protocol

A received envelope that does NOT match the `^Ping (\d+)$` regex
on the harness side MUST be delivered to the script interpreter.
If the script has no `expect` for the body within 250ms AND no
`ignore` is in effect that matches the body, the harness MUST
exit non-zero with `expect_drop_unmatched` rather than silently
drop the message. This is consistent with the project's
alpha-defaults policy (no silent fallbacks; the operator and the
CI failure log need a structured signal). `ignore` is the
explicit opt-in for scripts that DO want to permit chatty
background traffic.

## Risks / Trade-offs

- **Risk: harness hangs on a slow assertion.** Mitigation: every
  `expect` has an explicit timeout; harness exits non-zero on timeout.
- **Risk: harness interferes with a real Coordinator session.** Mitigation:
  the harness lives in `agentmux-test` namespace; production traffic
  that does NOT have `send` scope `all` cannot reach `*@<test_bundle>`
  (existing data-driven authorization spine). The
  `test-isolated=true` filter on `agentmux host relay` autostart
  keeps production relays from inadvertently hosting the test bundle.
- **Risk: scripted flows miss real-world variance.** Acknowledged
  limitation. The harness is a regression-test tool, not a substitute
  for live testing. Live Coordinator sessions still drive the
  weekly release-validation flow.
- **Risk: gate on `issues/acp/12` resolution.** If ACP tool
  advertisement is unstable, scripted tests will flake. Mitigation:
  the gate is in the proposal; if ACP doesn't stabilize, the
  proposal can be scoped down to TUI-driven tests only. The
  gate's exit criterion is documented explicitly in `proposal.md`
  (root cause AND a `procedures/` workaround note).

## Migration Plan

This is a net-new capability. No migration of existing tests is
required. Existing e2e tests (driven by live Coordinator sessions)
continue to work; the harness is an addition, not a replacement.

## v1 Scope Boundary (anti-scope-creep note)

The harness v1 is a **Coordinator simulator with a four-operation
script grammar**, sufficient to express the three v1 regression
tests (relay-25-conformance, sigterm-choreography,
cross-relay-attribution). It is NOT a general-purpose test framework
and it is NOT a parallel Agentmux runtime. The "Open Questions"
section below lists aspirations that we explicitly defer to v2 or
later; they each require their own OpenSpec proposal before landing
and should not be silently absorbed into v1 implementation.

The v1 implementation should fit in roughly:

- One new CLI subcommand tree (`src/commands/test/`).
- One new module (`src/test_harness/` -- relay peer + script parser
  + script interpreter + protocol constants).
- A handful of edits to `src/runtime/bootstrap.rs` and
  `src/relay/watcher.rs` to wire the `test-isolated` flag and the
  autostart filter.
- A handful of unit and integration tests.

If the v1 implementation grows beyond that envelope, the
complexity has crept past the v1 scope and a fresh proposal is
warranted.

## Open Questions

- In-process relay peer vs. CLI subprocess invocations. The
  current text assumes an in-process peer in `src/test_harness/relay_peer.rs`.
  If the v1 scripted tests can be expressed via CLI subprocess
  invocations without unacceptable latency, that is the simpler
  v1 path and `relay_peer.rs` may not be needed. Decision will be
  made during task 3.1. (v1 implementation decision; not a
  scope expansion.)
- Should the harness expose its script state via a `look`-style
  snapshot? (Useful for debugging hung tests.) **Deferred to v2;
  requires a fresh OpenSpec proposal.**
- Should the harness support parallel scripted flows? (Useful for
  load testing.) **Deferred to v2; requires a fresh OpenSpec
  proposal.**
- Should the harness be runnable as a library (Rust API) for
  in-process testing? **Deferred to v2; requires a fresh OpenSpec
  proposal.**
- LLM-backed Coordinator simulation. **Deferred; out of v1 scope
  per the proposal's Non-Goals section. Requires a fresh OpenSpec
  proposal if pursued.**
