# ACP Prime Timeout and Wedge Detection — Design Notes

This file captures the technical decisions and rationale for the
`acp-prime-timeout-and-wedge-detection` proposal. It complements
`proposal.md` (which states the what/why/impact) and the spec deltas
(which state the normative requirements).

## Context

The Tmux wedge-detection proposal (merged as `tmux-wedge-detection`)
introduced a generic `DeliveryEnvelope.prime_timeout_ms` field and a
per-coder `prime-timeout-ms` config key under `[coders.<id>.tmux]`,
explicitly deferring the ACP delivery-side analogue to this follow-up
(`design.md` Decision 7 / Future Work).

ACP delivery has no bounded prime window today. The ACP delivery task
(`src/acp/transport.rs::acp_delivery_task`) blocks on
`wait_for_prompt_complete` with a 100ms poll interval and only
exits when (a) the prompt resolves with a terminal `PromptCompletion`,
(b) `shutdown_requested()` fires, or (c) the next coalesce iteration
runs `try_recv` and observes a `WriteItem::Raw` or empty. A target
whose ACP child process has wedged (stdin buffer full, process stuck
without exiting, upstream service hung mid-turn) is waited on
indefinitely. The only bounded outcome is relay shutdown — same shape
as the Tmux unbounded behavior `tmux-wedge-detection` retires for
Tmux sessions.

The operator-visible knob (`AcpTargetConfiguration.turn_timeout_ms`)
is already declared, already validated
(`src/configuration/targets.rs:248-253`), and already plumbed through
the raw loader (`src/configuration/raw.rs:47,79`) — but it is dead
baggage. Nothing in `src/acp` reads it. Nothing in
`src/relay/delivery/dispatch/worker.rs::build_coder_envelope` threads
it into the `DeliveryEnvelope` for ACP targets.

## Goals

- Land the prime-timeout half of the ACP parity: bounded prime wait
  on the ACP delivery task, configurable per-coder under
  `[coders.<id>.acp].prime-timeout-ms`, with a distinct
  operator-visible outcome (`SendOutcome::Timeout` +
  `reason_code = "acp_turn_timeout"`).
- Match the Tmux operator vocabulary exactly:
  `[coders.<id>.acp].prime-timeout-ms` is symmetric with
  `[coders.<id>.tmux].prime-timeout-ms` (no `acp-` prefix; the table
  namespaces the key).
- Match the Tmux config-only v1 posture: drop the pre-existing
  per-call override surfaces (`--acp-turn-timeout-ms` CLI flag,
  `acp_turn_timeout_ms` MCP payload field) entirely so v1 has no
  transport-specific timeout field on any generic interface.
- Use the generic `DeliveryEnvelope.prime_timeout_ms` field
  introduced by `tmux-wedge-detection`. Do NOT introduce a
  transport-prefixed envelope field.
- Provide a deterministic, fast test surface (integration tests +
  unit tests on the prime-timer anchor) that proves the timer fires,
  coalesce does not reset it, and the operator interaction (pending
  choice) suppresses firing while active.

## Non-Goals

- ACP delivery-side wedge detection (deferred — see Decision 3).
- Per-call ACP operator timeout override. v1 has no per-call
  surface; both Tmux and ACP are fully config-only.
- New `SendOutcome` variants. The new outcome reuses
  `SendOutcome::Timeout` and the existing `acp_turn_timeout` reason
  code from the `ACP Stop-Reason Outcome Mapping` requirement
  (`session-relay/spec.md:1463`).

## Decisions

### Decision 1: Rename `turn_timeout_ms` to `prime_timeout_ms` for cross-transport symmetry

The pre-existing ACP operator knob is `turn_timeout_ms`, surfaced as
the TOML key `[coders.<id>.acp].turn-timeout-ms`. The Tmux wedge-
detection proposal landed the equivalent Tmux knob under
`[coders.<id>.tmux].prime-timeout-ms` — different name, different
operator vocabulary.

Operator feedback on this proposal (2026-06-30) directs the rename:
the ACP operator knob becomes `prime_timeout_ms`, surfaced as the
TOML key `[coders.<id>.acp].prime-timeout-ms`. The two surfaces are
then symmetric — same key name, same meaning (per-coder prime
wait bound), different tables.

Why rename rather than keep the legacy name?

1. **Operator vocabulary symmetry.** Operators configuring both
   Tmux and ACP coders see the same key (`prime-timeout-ms`) on
   both sides. The mental model is "the table namespaces the
   transport; the key names the deadline" — identical for both
   transports. The pre-existing `turn-timeout-ms` name was an
   artifact of the original ACP MVP proposal
   (`add-acp-send-runtime-mvp`) and predates the
   transport-decoupling work.
2. **No production consumers read the legacy name today.**
   `rg turn_timeout_ms src/` finds only the configuration
   declaration and validator — no runtime consumer. The "preserve
   backwards compatibility" argument does not apply to a field
   that nothing reads; the cost of the rename is operator
   vocabulary churn, not broken production behavior.
3. **Alpha-defaults apply.** Per AGENTS.md "Alpha Defaults": this
   project is alpha software with live releases; backwards
   compatibility is preserved only when the human developer
   explicitly requests it. Operator direction on this proposal is
   the explicit request to break the legacy vocabulary in service
   of cross-transport symmetry.

The rename touches:

- `src/configuration/types.rs:158` — typed field
- `src/configuration/raw.rs:47,79` — raw + intermediate loader
- `src/configuration/targets.rs:71,248-253,311` — builder,
  validator, validator copy

The runtime never reads `turn_timeout_ms` today, so no production
code path changes beyond the configuration rename. The validator's
error message updates from "ACP turn-timeout-ms must be greater
than zero" to "ACP prime-timeout-ms must be greater than zero."

### Decision 2: Drop the pre-existing per-call override surfaces

The Tmux wedge-detection proposal retired the
`--quiescence-timeout-ms` CLI flag and the `quiescence_timeout_ms`
MCP payload field — both transport-specific timeout fields
reachable through the generic `send` interface. The ACP equivalents
(`--acp-turn-timeout-ms`, `acp_turn_timeout_ms`) were outside that
proposal's scope; they remained in place through the
`tmux-wedge-detection` change.

Operator feedback on this proposal (2026-06-30) directs the
retirement: drop `--acp-turn-timeout-ms` and `acp_turn_timeout_ms`
entirely so v1 has no transport-specific timeout field on the
generic `send` interface. Operators who need a per-call override
do not have one in v1; they configure the per-coder
`prime-timeout-ms` key.

Why drop rather than keep?

1. **Generic-interface purity.** `--acp-turn-timeout-ms` and
   `acp_turn_timeout_ms` are the only transport-specific timeout
   fields remaining on `send` after `tmux-wedge-detection`. Keeping
   them means the generic interface carries a per-transport escape
   hatch forever. Operator direction is that this escape hatch is
   the wrong shape — the generic interface should be free of
   transport-specific fields.
2. **Per-call override is rarely needed in practice.** Operators
   who need a non-default deadline usually have a small set of
   distinct coder profiles (long-running autonomous agents,
   fast-turnaround human-in-the-loop flows, etc.). The per-coder
   config key covers this need without polluting the per-call
   interface. Operators who genuinely need a one-off override can
   edit the bundle config and restart — acceptable in alpha.
3. **Operator vocabulary symmetry.** With the rename in Decision
   1, both transports are config-only. Reintroducing per-call
   overrides later would require deciding "per-call override of
   which field?" — the ACP field would naturally be
   `--acp-prime-timeout-ms` / `acp_prime_timeout_ms`, parallel to
   the new config key. Defer that decision until operators report
   a concrete need; v1 is config-only.

The retirement touches:

- `src/relay/handlers/send.rs` — drop the read of
  `acp_turn_timeout_ms` from the request payload into the
  `QuiescenceOptions` constructor. After the retirement, the
  constructor takes only `quiet_window_ms` for ACP sessions.
- `src/cli/` — drop the `--acp-turn-timeout-ms` flag definition
  and the validator branch that maps the flag to the
  `acp_turn_timeout_ms` payload field.
- Operator-facing docs — remove the flag from the `agentmux send`
  help text.

Existing CLI invocations that pass `--acp-turn-timeout-ms` will
fail at the CLI parser as unknown. Existing MCP payloads that
include `acp_turn_timeout_ms` will be rejected as unknown.
Operator direction accepts both breaks.

### Decision 3: Prime timer anchor is "delivery task perspective, first wait start"

Mirroring `tmux-wedge-detection` Decision on the Tmux prime timer:

- The prime timer starts when `submit_envelope_turn` first calls
  `wait_for_prompt_complete`. Not at enqueue time (so queue depth
  does not count against the operator's deadline).
- The prime timer resets when the worker transitions out of `Busy`
  into any other state. Today the only such transition on the
  per-turn code path is `Busy -> Available` after a terminal
  completion is observed. The new code path is
  `Busy -> Unavailable` on a prime-timer fire, with the flush
  group resolved as `Timeout` and the per-target readiness latched
  to `Unavailable` so the worker's respawn-needed signal can
  re-bootstrap the runtime (matching the existing
  `signal_respawn_if_needed` path on
  `PromptCompletion::ConnectionClosed`).
- The prime timer does NOT reset on coalesce-during-wait. If a
  new envelope is absorbed into the same flush group via
  `WriteItem::Envelope` in the coalesce loop, the absorbed
  envelope inherits the head envelope's prime timer anchor (it
  does not extend or restart the deadline). This matches
  `tmux-wedge-detection` Decision on the Tmux prime timer.

The head envelope's `prime_timeout_ms` governs the entire flush
group's prime wait. A later envelope's `prime_timeout_ms` does
not extend or shorten a wait already in progress.

### Decision 4: Wedge detection is deferred, with explicit rationale

The Tmux wedge model is "settled + not prompt-ready + no operator
interaction" — equivalent to the ACP wedge model proposed in the
original `tmux-wedge-detection/design.md:177-194` as
"`WorkerReadinessState::Available` AND no outstanding pending
choice request".

Coordinator guidance on this proposal (2026-06-30) corrected this
analogy: `WorkerReadinessState::Available` is the ACP **success**
state (the agent completed a turn cleanly), not a wedge indicator.
A predicate that classifies on `Available` would produce
false-positive wedges on every successful ACP turn — a clear
correctness bug.

Coordinator's framing for a clean v1 ACP wedge predicate: "ACP
server not accepting new requests on its stdin channel" — i.e. a
stdin-acceptability probe that observes the child's stdin write
queue and reports a wedged state when writes are blocked without
progress. That probe trait does not exist today and is non-trivial
to add correctly:

1. ACP writes are non-blocking on the relay side
   (`write_line_to_stdin` in `src/acp/client.rs:530`) and the
   existing retry-on-`TextFileBusy` path in
   `AcpStdioClient::spawn_command` (commit `06b3bf9`) handles the
   spawn-side race. A "stdin wedged mid-turn" probe is a different
   surface (post-spawn, mid-turn) and requires new instrumentation.
2. The probe must not classify a turn that is genuinely running
   (the agent is doing tool work, not accepting new requests) as
   wedged. That requires either a per-RPC inflight detection or a
   distinguishing predicate that the current
   `WorkerReadinessState` model does not carry.
3. The probe's test surface (the five canonical sequences in
   `tmux-wedge-detection` Decision 6) does not have an obvious
   ACP mapping. ACP delivery is fundamentally different from pane
   quiescence; the sequences need to be re-thought, not adapted.

This proposal therefore **defers** ACP wedge detection to a
separate change. The deferral is recorded explicitly here and in
`proposal.md` so the wedge symmetry question is preserved for
future work. The future-work note names the predicate shape
(stdin acceptability), the missing probe trait, and the test
surface requirements — sufficient that the next wedge proposal
does not have to re-derive the rationale.

### Decision 5: Outcome mapping preserves the existing `acp_turn_timeout` reason code

The `ACP Stop-Reason Outcome Mapping` requirement
(`session-relay/spec.md:1463-1482`) already defines the
`acp_turn_timeout` reason code as the canonical timeout reason on
ACP. This proposal reuses that reason code; it does NOT introduce
a new reason code or a new `SendOutcome` variant.

The relay worker (`src/relay/delivery/dispatch/worker.rs`) and
the stream event payload (`delivery_outcome` event in
`session-relay/spec.md` "Relay Stream Event Contract") already
surface `outcome = "timeout"` with arbitrary `reason_code` values.
No relay-worker edits are required.

### Decision 6: Active operator interaction (pending choice) suppresses prime-timer fire

While a `pending_choice_outcome` slot is populated with a `None`
signal at the start of a turn (the "chooser is parked, waiting
for operator decision" state), the prime timer does NOT fire. The
reasoning mirrors `tmux-wedge-detection` Decision 4 (operator
interaction indefinitely suppresses both `unresponsive` and
`wedged` classification): a pending operator choice is a
legitimate, long-running state, and firing `Timeout` on a chooser
that the operator is mid-decision is a false positive.

Concretely: the ACP delivery task checks `pending_choice_outcome`
before firing the prime timer. If a choice is in flight (the
`on_permission` handler ran, the `pending_choice` mutex holds
`None`), the prime timer continues to wait without firing. The
timer only resumes firing if the choice resolves
(`ChoiceMade::Chosen` or `ChoiceMade::Cancelled`) and the
underlying turn subsequently does not produce a terminal
completion.

This matches the existing choice-resolution contract
(`session-relay/spec.md` Non-Expiring Choice Pending Lifecycle)
and the existing `build_acp_completion_result` flow, which
already reads `pending_choice_outcome` to map choice cancellation
to a failed outcome.

### Decision 7: Prime timer fire does NOT inject, but does signal respawn-needed

On prime-timer fire, the ACP delivery task resolves the flush
group with `Timeout` + `reason_code = "acp_turn_timeout"`, sets
the per-turn readiness to `Unavailable`, and emits a
`delivery_prime_timeout` inscription with `target_session`,
`timeout_ms`, and `prime_wait_elapsed_ms`. The respawn-needed
signal mirrors the existing `signal_respawn_if_needed` path on
`PromptCompletion::ConnectionClosed`: the worker's
`AcpWorkerDriver::check_respawn_needed()` returns `true`, and the
driver triggers respawn via the same path used for
connection-closed failures.

This matches the existing `tmux-wedge-detection` Decision that
`delivery_prime_timeout` inscriptions are emitted on Tmux fire.

## Future Work (Not Part of This Proposal)

### ACP delivery-side wedge detection

A future OpenSpec would introduce:

- An ACP wedge probe trait
  (`AcpStdioClientWedgeProbe`-style) in `src/acp/client.rs` or
  `src/acp/transport.rs`, mirroring the transport-internal probe
  seam pattern from `tmux-wedge-detection` Decision 6. The probe
  would return either an "accepting writes" or a "wedged on
  stdin" signal on demand, and the delivery task would consume it
  during the prime wait to classify the flush group.
- A canonical sequence test surface (analogous to the Tmux five
  canonical sequences), with at minimum:
  - `AlwaysWedgeProbe` — ACP stdin perpetually unwritable;
    asserts `Failed` + `reason_code = "agent_wedged"`.
  - `NormalTurnProbe` — prompt completes cleanly; asserts
    `Delivered` without wedge firing.
  - `PendingChoiceProbe` — choice in flight; asserts neither
    timeout nor wedge firing while operator interaction is
    active.
- A `DeliveryWaitError::Wedged { reason: String }` variant
  (mirrors the Tmux-side variant added by
  `tmux-wedge-detection` Section 3.1) and an outcome mapping to
  `SendOutcome::Failed` + `reason_code = "agent_wedged"`.

The wedge predicate is the load-bearing design decision for that
future proposal. The probe-trait seam is the testability seam.
Without a clean predicate, wedge detection cannot land as
correctness-positive code; the deferral here is to avoid
implementing the wrong predicate.

### Per-call timeout override (deferred to operator demand)

If operators report a need for a per-call timeout override
distinct from the per-coder config, a future OpenSpec would
introduce per-call surfaces in a transport-neutral shape:

- MCP `send` payload: `prime_timeout_ms: Option<u64>` (no
  `acp_` / `tmux_` prefix — the field is generic across
  transports).
- CLI: `--prime-timeout-ms <MS>` (no `acp-` / `tmux-` prefix).
- Population rule: per-call override (highest) → per-coder
  config (default) → `None` (unbounded).

The transport-neutral shape is intentional. A per-call override
named `--acp-prime-timeout-ms` would re-introduce the
transport-specific escape hatch this proposal retires; a generic
`--prime-timeout-ms` honors the same operator vocabulary
symmetry that drives Decision 1.

This is recorded as future work, not as a deferred lock-step
follow-up: the decision to reintroduce per-call overrides
should be driven by operator demand, not by symmetry alone.

## Risks / Trade-offs

- **Risk:** the operator knob rename breaks bundle configs that
  had `turn-timeout-ms` set. **Mitigation:** the rename is the
  operator direction (Decision 1); alpha defaults apply; the
  raw loader's `deny_unknown_fields` produces a clear bundle
  load error pointing operators at the new key name.
- **Risk:** the per-call override retirement breaks `send`
  invocations that pass `--acp-turn-timeout-ms` or
  `acp_turn_timeout_ms`. **Mitigation:** the retirement is the
  operator direction (Decision 2); CLI invocations fail at the
  parser with an unknown-flag error; MCP payloads fail with an
  unknown-field error. Alpha defaults apply: the rejections are
  generic and do NOT name a replacement. Operators who hit the
  rejection consult the changelog.
- **Risk:** false-positive timeouts on legitimately slow agents
  whose turns exceed the configured `prime-timeout-ms`. Same risk
  as the Tmux prime timeout, same mitigation: opt-in (default
  `None` preserves today's unbounded behavior; operators opt in
  by setting a finite value).
- **Risk:** the prime timer fires mid-choice if a long-running
  choice is in flight and the operator is slow to decide.
  **Mitigation:** Decision 6 — the prime timer does not fire
  while `pending_choice_outcome` is in flight. The cost of a
  slow operator choice is the operator's, not the prime timer's.
- **Risk:** wedge detection is left to a future proposal and the
  silent-wedge failure mode (stdin stuck, no observable signal)
  remains unbounded. **Mitigation:** the existing
  `AcpStdioClient::spawn_command` retry-on-`TextFileBusy` path
  (commit `06b3bf9`) covers the spawn-side race. Mid-turn stdin
  wedge is genuinely rarer than spawn-time race and is not a
  daily-fire failure mode; deferring wedge detection while
  shipping prime timeout is the correct priority order.

## Migration Plan

1. Land OpenSpec deltas for `session-relay` (ADDED ACP Prime
   Timeout requirement; MODIFIED Non-Expiring Choice Pending
   Lifecycle requirement to remove the dead parenthetical),
   `transport-abstraction` (ADDED ACP Prime Timeout Envelope
   Field Consumption requirement), `cli-surface` (MODIFIED Send
   Timeout Override Flags by Transport to remove the per-call
   ACP timeout flag), and `mcp-tool-surface` (MODIFIED Send
   Target Selection to remove the per-call ACP timeout payload
   field).
2. Rename `AcpTargetConfiguration.turn_timeout_ms` →
   `AcpTargetConfiguration.prime_timeout_ms` across
   `src/configuration/types.rs`, `src/configuration/raw.rs`, and
   `src/configuration/targets.rs`. Update the validator's error
   message accordingly.
3. Drop the per-call ACP timeout surfaces:
   - `src/relay/handlers/send.rs` — remove the read of
     `acp_turn_timeout_ms` from the request payload.
   - `src/cli/` — remove the `--acp-turn-timeout-ms` flag
     definition.
   - Operator-facing docs — remove the flag from the
     `agentmux send` help text and from any `MCP send` payload
     documentation.
4. Wire `AcpTargetConfiguration.prime_timeout_ms` into
   `DeliveryEnvelope.prime_timeout_ms` in
   `src/relay/delivery/dispatch/worker.rs::build_coder_envelope`.
   Only the ACP path populates the new field; the Tmux path is
   unchanged (the Tmux proposal already populates from
   `[coders.<id>.tmux].prime-timeout-ms`).
5. Update `src/relay/delivery/quiescence.rs::QuiescenceOptions`
   to carry the new `prime_timeout_ms: Option<u64>` constructor
   parameter (this is already done by the Tmux wedge-detection
   proposal; the ACP path reads from it).
6. Update `src/acp/transport.rs::acp_delivery_task` /
   `submit_envelope_turn` to:
   - Read `envelope.prime_timeout_ms` at turn start.
   - Track the prime timer anchor (start time when first wait
     begins; do not reset on coalesce-during-wait).
   - On each `wait_for_prompt_complete` poll, check whether the
     prime window has elapsed AND no `pending_choice_outcome` is
     in flight AND no `PromptCompletion` has been observed.
   - On fire, resolve the flush group with
     `SendOutcome::Timeout` + `reason_code = "acp_turn_timeout"`,
     set readiness to `Unavailable`, emit a
     `delivery_prime_timeout` inscription, and signal
     respawn-needed.
7. Add integration tests:
   - `acp_prime_timeout_fires_after_configured_window` — sets
     `prime-timeout-ms` to a short value, submits a prompt to a
     child that never responds, asserts `Timeout` outcome +
     `acp_turn_timeout` reason code +
     `delivery_prime_timeout` inscription.
   - `acp_prime_timer_does_not_reset_on_coalesce` — sets
     `prime-timeout-ms` to a finite value, submits two
     envelopes into the same flush group, asserts the second
     envelope inherits the head envelope's prime anchor (does
     not extend the deadline).
   - `acp_prime_timer_does_not_fire_during_pending_choice` —
     sets `prime-timeout-ms` to a finite value, raises a
     permission request mid-turn, asserts the prime timer does
     not fire while the choice is pending.
8. Add unit tests for the prime-timer anchor calculation
   (coalesce-during-wait does not reset; choice pending does
   not fire).
9. Document the operator-visible knob rename in
   `src/acp/README.md` (if present) and the operator-facing
   bundle config docs: `[coders.<id>.acp].prime-timeout-ms` is
   the new key name; `turn-timeout-ms` is no longer accepted;
   the field is opt-in (default `None` preserves today's
   unbounded behavior).
10. Document the per-call override retirement in the
    `agentmux send` help text and the MCP `send` payload docs:
    `send` carries no per-call timeout override field in v1; the
    per-coder config is the only timeout surface.

## Open Questions

- Should the prime timer fire reset the per-target readiness to
  `Unavailable` (current Decision 7) or to `Recovering` (matching
  the post-respawn state)? **Recommendation:** `Unavailable`,
  matching `PromptCompletion::ConnectionClosed` and the
  existing `signal_respawn_if_needed` path. `Recovering` is the
  post-respawn state; prime-timer fire is a pre-respawn state.
  **Locked decision in this proposal: `Unavailable`.**
- Should the prime timer fire cancel the in-flight prompt via
  `client.cancel()` (if the ACP server supports it) or just
  abandon the wait? **Recommendation:** abandon the wait — the
  prompt may still resolve and we should not assume the server
  honors cancellation. The transport resolves the flush group
  with `Timeout` and signals respawn; the relay does not inject
  further messages until the worker respawns. **Locked decision
  in this proposal: abandon the wait, do not cancel.**
- Should the prime timer fire be suppressed when the target's
  `WorkerReadinessState` is already `Initializing` or
  `Recovering` (rather than `Busy`)? **Recommendation:** yes —
  if the worker is mid-bootstrap or mid-respawn, the prompt
  may legitimately take longer than the prime window to be
  accepted. The prime timer only counts down after the prompt
  is dispatched (`PromptDispatchOutcome::Submitted`); the
  dispatch path's own acceptance is bounded by
  `AcpStdioClient::request`'s `RecvTimeoutError::Timeout` at
  `src/acp/client.rs:541`, which is a separate concern from the
  prime-window wait. **Locked decision in this proposal: prime
  timer starts only after
  `PromptDispatchOutcome::Submitted`.**