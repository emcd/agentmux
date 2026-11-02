# Round-trip equivalence drafts (conformance task 4.1)

Status: drafts. Each draft promotes to an executable conformance case when
both of its execution dependencies land: the canonical operation (tool-ops,
after step-3 Phase 1 delivers the runtime handle + verified-context types)
and the reference adapter binary. Until then no draft executes; the
exclusion rule below already binds them.

Shared assertion shape (adapted-is-direct): for a fixed operation input plus
fixed relay state, the wire payload produced through the adapter path MUST
equal the payload produced by direct canonical-operation invocation, field
for field, with reason-code namespacing (`acp.*` vs relay vocabulary)
preserved across the mapping and never collapsed. MCP request/response
shapes appear in no draft.

## Draft 1: submit-content equivalence

- Wire method: `adapter.submit` (supervisor-to-adapter transport of an
  authorized dispatch outcome, never an operation call).
- Direct counterpart: the canonical `send` operation over the public
  runtime handle + relay-verified principal context.
- Setup: fixed `send` input (targets + operation-bundle param, envelope)
  against fixed relay state; the relay core runs validation + authorization
  centrally and the resulting delivery obligation is what the supervisor
  submits.
- Assertion: the submit params render exactly the authorized dispatch
  (target/bundle qualification, envelope, correlation_id); the adapter
  executes and acks per the trichotomy (`transport_ack` /
  `SubmissionUnknown`, never re-authorizing, never synthesizing
  semantics). For the fixed input + state, submit frame MUST carry
  exactly the operation outcome's content.

## Draft 2: choice round-trip equivalence

- Wire method: `adapter.requestChoice` (adapter-to-relay reverse RPC).
- Direct counterpart: the canonical `decide` operation — the same `decide`
  an in-process `choose` caller invokes, with target/message/decider
  authorization derived from relay state (adapter-provided fields confer
  nothing).
- Setup: two isolated equivalent pending choices, one resolved via wire
  round-trip, one via direct decide invocation, for the same input +
  verified context.
- Assertion: both resolutions carry the same outcome, and the wire result
  nests it under exactly one key — `{"chosen": {"option_id",
  "decided_by"}}` or `{"cancelled": {"decided_by"?, "reason_code",
  "reason"?}}` with `correlation_id` echoed (both keys present or neither
  is a framing failure). Covers the spoofed-decider negative:
  an adapter-presented decider the relay did not authorize is rejected
  rather than applied.

## Draft 3: look-poll equivalence

- Wire method: `relay.look` poll (supervisor-to-adapter).
- Direct counterpart: the canonical `look` operation (target,
  lines/offset only — no timeout on the operation surface).
- Setup: fixed look input against a target with a known snapshot; the
  relay-supplied prime timeout, stable look-handle, and generation
  binding are adapter/supervisor wire context, not caller tool input.
  Poll reply renders from the same snapshot semantics as a direct look
  call, including superseded-generation discard (an old-generation reply
  loses to a newer snapshot). Scripted snapshots carry an availability latch
  (`delay_ms`, default 0): fresh answers only once first-poll time plus
  delay has elapsed on the injected clock; earlier polls get stale-tagged
  priming immediately and the loop never blocks, so fence and cancel
  traffic keeps flowing.
- Assertion: poll reply carries the same entries + freshness metadata
  as the direct call: `freshness` is exactly one of
  fresh|stale|initializing, `snapshot_source` exactly one of
  live|cache|synthetic|none (never interchanged), plus `entries_total`,
  `returned_entries_count`, and the stale/discontinuity flags;
  synthetic marks carry `stale=true`, never fresh. Latch directions
  (prime timeouts are wire policy, not operation input): delay
  200/prime-timeout 2000 polls stale priming immediately, then fresh
  after first-poll plus 200 elapses; prime-timeout 200 with delay 2000
  returns stale priming within the timeout.

## Exclusion rule (binds drafts and future cases)

No equivalence case for `help` (no operation), supervision-only methods
(`adapter.handshakeHello`, `adapter.persistState`, `adapter.lifecycleEvent`,
`relay.supervisionSignal`), the `relay.ping` liveness probe, or
adapter-local errors (not in the canonical error set). A ping/pong
protocol fixture is still required outside the equivalence set.

## Persist payload-richness limit (Tier-2 review constraint)

Persist origination is per completed turn with a scripted turn-marker
blob (`{"last_turn":"..."}`). Behavioral cases (tag echo, refusal/commit
handling, persist-unknown at teardown) execute now; the limit excuses
exactly one thing: rich-content resume assertions against the scripted
turn-marker fixture wait for the resume-state follow-on. It excuses no
protocol or lifecycle defect and no genuinely invalid content. Fixture
headers cite this limit alongside the requirement citations so it
travels with the fixtures.

## Seam-exercise rule (binds future cases)

Any case added at a shape-rule-vs-admission-rule seam is exercised against
the already-stated rules for that class before filing as an exemplar.
