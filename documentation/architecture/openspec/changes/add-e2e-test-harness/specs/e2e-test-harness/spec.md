## ADDED Requirements

### Requirement: Cross-Namespace Reachability to Test Bundles

The system SHALL apply the existing data-driven authorization spine
(`specs/relay-routing-layer/spec.md` and `specs/session-relay/spec.md`)
to sends targeting `*@<test_bundle>` -- the requester's configured
`send` scope against the uniform threshold (`self` / `home` / `all`)
governs reach, exactly as for any other cross-bundle send. The harness
introduces NO recipient-side gate, NO per-operation cross-namespace
exception, and NO new policy flag for the test-bundle case.

The test bundle's safety story relies on three pre-existing
properties of the runtime, not on a new gate:

- `test-isolated=true` keeps `agentmux host relay` from auto-hosting
  the test bundle, so production relays do not inadvertently run it.
- A CI test runner's session is granted `send` scope `all` by the CI
  operator's relay policy, exactly as a Coordinator session is.
  Production senders without `send` scope `all` cannot reach
  `*@<test_bundle>` from a different namespace; that is the
  requester-side policy, not a test-specific gate.
- The harness's script interpreter is the only thing that listens on
  the harness principal; envelopes not matching an `expect` or an
  `ignore` operation are surfaced as `expect_drop_unmatched` rather
  than silently dropped (see Harness Auto-Ack Protocol).

#### Scenario: Cross-namespace send to test bundle under existing authorization

- **GIVEN** a CI test runner session in `agentmux-ci` with `send`
  scope configured as `all`
- **AND** a test bundle `agentmux-test` hosted by `agentmux test
  bundle up` with `test-isolated=true`
- **WHEN** the CI test runner sends to `qa-harness@agentmux-test`
- **THEN** the relay accepts the send under the requester's `send`
  scope `all` -- the same authorization path that any other
  cross-bundle send takes
- **AND** delivers the envelope to the harness principal

#### Scenario: Cross-namespace send to test bundle denied under existing authorization

- **GIVEN** a production session in `agentmux` with `send` scope
  configured as `home`
- **WHEN** the session sends to `qa-harness@agentmux-test`
- **THEN** the relay rejects the send with `authorization_forbidden`
  -- the existing `home` vs `all` threshold check, NOT a
  test-bundle-specific gate

### Requirement: Harness Script Grammar v1

The harness script grammar v1 SHALL support five operations:

- `expect <pattern> <timeout_ms>`: wait for an inbound envelope whose
  body matches the regex `pattern` (Rust `regex` crate syntax); fail
  if no match within `timeout_ms`. Patterns are anchored at the start
  of the body (`^` is implicit). Capture groups use `(...)` syntax,
  1-based numbering, and are stored as `last_expect[N]` for use by
  `assert_equal`.
- `send <target> <body>`: send `body` to the canonical session id
  `target` via `agentmux_send`. `target` is resolved via the same
  canonical-identifier rules as `agentmux_send`.
- `assert_equal <expected> <actual>`: `actual` MUST be either
  `last_expect` (the full body of the most recent matched `expect`)
  or `last_expect[N]` (the Nth capture group, N >= 1). Any other
  expression is rejected with `unsupported_assert_expr`. Exit
  non-zero with `assertion_failed` if the values do not equal
  `expected`.
- `raise_signal SIGTERM <pid>` (test only): issue SIGTERM to the
  process at `pid`. The pid MUST be a harness-registered test
  process; signals to other pids are rejected with
  `harness_signal_unauthorized`.
- `ignore <pattern>`: explicitly opt in to silent drop for any
  envelope whose body matches `pattern` (Rust `regex` crate syntax,
  same as `expect`). An `ignore` operation MUST be in effect (i.e.
  the script has run it and not yet advanced past it) at the time
  a non-ping envelope arrives, OR the harness MUST exit non-zero
  with `expect_drop_unmatched`. `ignore` is the only operation that
  permits a non-ping envelope to be silently dropped; without it,
  unexpected traffic is a hard failure.

A script is a TOML file with a top-level `operations: [...]` array.
Operation order is the order in the array.

#### Scenario: expect times out

- **WHEN** the script runs `expect "Pong 5" 1000`
- **AND** no matching body is received within 1000ms
- **THEN** the harness exits non-zero with the reason
  `expect_timeout pattern="Pong 5" timeout_ms=1000`

#### Scenario: expect matches

- **WHEN** the script runs `expect "Pong 5" 1000`
- **AND** the harness receives an envelope with body `Pong 5` within
  1000ms
- **THEN** the harness advances to the next operation
- **AND** the assertion state is updated with the matched body

#### Scenario: expect with capture group

- **WHEN** the script runs `expect "^Pong (\\d+)$" 1000`
- **AND** the harness receives an envelope with body `Pong 42`
- **THEN** `last_expect` is set to `Pong 42`
- **AND** `last_expect[1]` is set to `42`

#### Scenario: assert_equal passes

- **WHEN** the most recent `expect` matched body `Pong 5`
- **AND** the script runs `assert_equal "Pong 5" last_expect`
- **THEN** the assertion passes
- **AND** the harness advances

#### Scenario: assert_equal on capture group passes

- **WHEN** the most recent `expect` matched `Pong 42`
- **AND** the script runs `assert_equal "42" last_expect[1]`
- **THEN** the assertion passes

#### Scenario: assert_equal fails

- **WHEN** the most recent `expect` matched `Pong 5`
- **AND** the script runs `assert_equal "Pong 6" last_expect`
- **THEN** the assertion fails
- **AND** the harness exits non-zero with the reason
  `assertion_failed expected="Pong 6" actual="Pong 5"`

#### Scenario: assert_equal with unsupported expression

- **WHEN** the script runs `assert_equal "foo" last_send.body`
- **THEN** the operation fails with `unsupported_assert_expr`
- **AND** the harness exits non-zero

#### Scenario: raise_signal to unauthorized pid

- **WHEN** the script runs `raise_signal SIGTERM 12345`
- **AND** pid 12345 is not a harness-registered test process
- **THEN** the operation fails with `harness_signal_unauthorized`
- **AND** the harness exits non-zero

#### Scenario: ignore permits silent drop of matching envelope

- **WHEN** the script runs `ignore "heartbeat \\d+"`
- **AND** an envelope with body `heartbeat 7` arrives while the
  `ignore` is in effect
- **THEN** the envelope is dropped silently (no `expect_drop_unmatched`
  failure)
- **AND** the script advances past the `ignore` once a non-matching
  envelope (or the next scripted operation) is reached

#### Scenario: ignore does not cover non-matching envelopes

- **WHEN** the script has run `ignore "heartbeat \\d+"`
- **AND** an envelope with body `something else` arrives
- **THEN** the `ignore` does NOT cover the body
- **AND** the script interpreter is invoked as if no `ignore` were in
  effect (the `expect_drop_unmatched` failure mode still applies if
  no other expect matches)

### Requirement: Harness Auto-Ack Protocol

The harness principal SHALL auto-acknowledge structured pings on the
`Ping N` -> `Pong N` protocol. A `Ping N` from any session is
responded to with `Pong N` (where `N` is the integer at the end of
the ping message), sent via `agentmux_send` to the original sender,
within 100ms of receipt. Envelopes whose body does NOT match
`^Ping (\d+)$` MUST be delivered to the script interpreter. If the
script has no `expect` for the body within 250ms AND no `ignore`
is in effect that matches the body, the harness MUST exit non-zero
with `expect_drop_unmatched` rather than silently dropping the
message.

#### Scenario: Harness replies to Ping N

- **WHEN** the harness receives an envelope whose body matches the
  regex `^Ping (\d+)$`
- **THEN** the harness sends `Pong N` (where N is the captured
  integer) back to the original sender via `agentmux_send` within
  100ms

#### Scenario: Non-ping message reaches the script

- **WHEN** the harness receives an envelope whose body does NOT match
  `^Ping (\d+)$`
- **THEN** the envelope is delivered to the script interpreter
- **AND** if the script has an `expect` that matches within its
  timeout, the script advances

#### Scenario: Non-ping message with no matching expect or ignore exits non-zero

- **WHEN** the harness receives an envelope whose body does NOT match
  `^Ping (\d+)$`
- **AND** the script has no `expect` for the body within 250ms
- **AND** no `ignore` is in effect that matches the body
- **THEN** the harness exits non-zero with the reason
  `expect_drop_unmatched body=<body> timeout_ms=250`

#### Scenario: Non-ping message covered by an active ignore

- **WHEN** the harness receives an envelope whose body does NOT match
  `^Ping (\d+)$`
- **AND** an `ignore` is in effect that matches the body
- **THEN** the envelope is silently dropped
- **AND** no `expect_drop_unmatched` failure is raised
