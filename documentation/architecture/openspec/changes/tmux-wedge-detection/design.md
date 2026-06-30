## Context

`TmuxTransport::wait_for_quiescent_pane` (in `src/tmux/transport.rs`) drives the
quiescence poll loop before each flush group. Today the loop runs forever — no
deadline, no wedge detection. Operators cannot distinguish a slow agent from a
hung pane, and the only existing terminal state is `Shutdown`. The structural
machinery for "the pane is settled at a non-prompt state" already exists in
the readiness-mismatch branch; it just keeps looping instead of resolving.

The UI transport already implements a bounded reconnect wait that produces
`SendOutcome::Timeout` (`src/transports/ui.rs`). The Tmux transport should
learn the same shape: bounded wait, distinct outcome variant, deterministic
test surface.

The ACP transport has a per-RPC `RecvTimeoutError::Timeout` path in
`src/acp/client.rs`, but no delivery-side prime window. ACP delivery-side
wedge detection is genuinely larger work — ACP delivery currently treats any
`WorkerReadinessState::Available` transition as terminal, with no notion of
"settled at a non-prompt state." A follow-up OpenSpec is required to land
that work; this proposal is Tmux-only for v1.

## Goals

- Three-state delivery classifier on the Tmux transport: `unresponsive`,
  `running`, `wedged`.
- Config-surfaced, opt-in prime timeout (`tmux-prime-timeout-ms`) and wedge
  detection (`tmux-wedge-detection`). Each defaults to disabled, preserving
  today's unbounded behavior on each knob independently.
- Distinct operator-visible outcomes for the two failure modes (`Timeout` vs.
  `Failed` with `reason_code = "pane_wedged"`).
- Deterministic, fast test surface via injected `PaneQuiescenceProbe` trait.
- Group-level atomicity: a wedged or unresponsive flush group fails as a whole.
  No per-message deadlines.
- Active operator-interaction signals (tmux copy-mode or key-table) suppress
  both `unresponsive` and `wedged` classification while active. Prime timeout
  does not fire while operator interaction is active.

## Non-Goals

- No ACP delivery-side prime timeout or wedge detection implementation.
- No per-call operator override (config-only for v1; per-call override is a
  backward-compatible follow-up if/when operators need it).
- No new `SendOutcome` variant. Both outcomes reuse existing variants.
- No relay-side changes — the relay worker already maps `Timeout` and `Failed`
  with arbitrary `reason_code` values; no `async_worker.rs` / `dispatch/worker.rs`
  edits are required.
- No changes to the `DeliveryEnvelope` schema (the existing
  `quiescence_timeout` field is repurposed on the Tmux side for the prime
  timeout; ACP will get its own field when the ACP follow-up lands).

## Decisions

### Decision 1: `quiescence_timeout` is repurposed on the Tmux side as the prime timeout

The `DeliveryEnvelope.quiescence_timeout` field already exists and is already
passed into `wait_for_quiescent_pane`. With the three-state model the wait's
"deadline" semantics shift from "bound on quiescence settling" to "bound on
the target producing *any* observable response during the quiescence wait."
On the Tmux side, the field is repurposed to mean the prime timeout. The
relay continues to leave the field as `None` for Tmux async delivery (today's
default), so operators opt-in via config.

This change requires the existing `Quiescence-Gated Delivery` requirement in
`session-relay` to be MODIFIED so its semantics are transport-specific:
on Tmux the field bounds the prime window (no observable output before the
timeout); on other transports the field MAY continue to bound the quiescence
wait per its existing semantics.

**Alternative considered:** rename the field to `prime_timeout_ms` in
`DeliveryEnvelope`. Rejected — the rename would force a wire-format/schema
touch and would not match the ACP delivery-side field that the follow-up will
introduce. Repurposing keeps the schema stable and matches the operator's
mental model: "a bounded-wait knob" with transport-specific semantics.

### Decision 2: Prime timeout fires before any quiescence pass, not after

The existing deadline check (`tmux/transport.rs` `wait_for_quiescent_pane`)
fires only after at least one quiescence pass. For prime timeout we need a
clock that starts at "first wait start" (delivery-task perspective — when the
flush actually begins, not when the envelope was enqueued) and fires when no
output has been observed by the deadline.

### Decision 3: Prime timer does NOT reset on coalesce-during-wait

Coalesce-during-wait absorbs new envelopes into the current flush group and
re-checks quiescence. The prime timer is anchored to first wait start and is
not extended by coalesced envelopes. Each envelope carries its own prime
timeout from the head envelope (consistent with `quiet_window` behavior
today).

### Decision 4: Wedge state is sticky once detected

When the classifier observes `quiescent AND not-prompt-ready AND no
operator-interaction-active`, the flush group resolves as wedged. The state
is not re-evaluated across coalesce iterations — once wedged, the flush group
fails atomically.

### Decision 5: `operator_interaction_active` indefinitely suppresses both classifiers

Today `operator_interaction_active` is checked only after the pane becomes
quiescent. For the three-state model we check it during the prime window AS
WELL. While `operator_interaction_active` reports an active copy-mode or
key-table for the target session, both `unresponsive` and `wedged`
classification are indefinitely suppressed — the transport continues to wait
and the prime timer does NOT fire.

This is the correct operational policy: an active operator interaction means
the user (or an external decision) is legitimately in progress. Firing
`unresponsive` while the user is in copy-mode would falsely classify the
target as failed; firing `wedged` while a `choose` request is pending
(classic Claude Code tool-approval flow) would falsely classify it as
wedged. The cost is that a perpetually-active operator interaction
indefinitely blocks delivery — but in that case the target is genuinely
blocked and the operator is expected to clear the interaction manually.

**Alternative considered:** prime timeout fires regardless of operator
interaction state. Rejected — produces false positives for any target with
a legitimately-pending decision.

### Decision 6: Group atomicity on failure modes

When prime timeout fires, the entire flush group fails as `Timeout`. When
wedge fires, the entire flush group fails as `Failed` with
`reason_code = "pane_wedged"`. This matches `paste_group`'s group-atomic
semantics. Per-message deadlines are explicitly out of scope.

### Decision 7: Probe trait for testability

Inject a `PaneQuiescenceProbe` trait into the Tmux wait path so tests can
drive deterministic probe results. The trait returns the next
`PromptReadinessEvaluation` on demand. Tests cover at minimum:

- `AlwaysUnresponsiveProbe` — never produces output. Asserts `Timeout`.
- `AlwaysWedgeProbe` — immediately quiesces at a non-prompt state. Asserts
  `Failed` + `reason_code = "pane_wedged"`.
- `PendingChoiceProbe` — quiesces with `operator_interaction_active = Some(_)`.
  Asserts neither timeout nor wedge; asserts the prime timer does NOT fire
  while operator interaction is active (transport continues to wait
  indefinitely).
- `SlowPromptProbe` — quiesces at a prompt state after several ticks. Asserts
  `Delivered`.
- `NormalFlowProbe` — produces output then settles at a prompt. Asserts
  `Delivered` without prime or wedge firing.

**Alternative considered:** end-to-end test with a real tmux session that
wedges (e.g. `cat` blocking on stdin). Rejected — slow, flaky, hard to
isolate; the probe trait gives deterministic coverage in milliseconds.

### Decision 8: ACP wedge model is future-work context only

The ACP wedge model would be `WorkerReadinessState::Available AND no
outstanding pending choice request` (analogous to tmux's "settled +
not prompt-ready + no `operator_interaction_active`"). The
`pending_choice_outcome` field on ACP transport state
(`src/acp/permission.rs`) carries the request signal.

The ACP wedge model is **not** added as an active `session-relay` SHALL
requirement in this proposal. The current proposal is Tmux-only for v1.
The ACP follow-up OpenSpec will land the ACP wedge and prime-timeout
requirements; this proposal captures the model here for design continuity
and to signal the follow-up's contract scope, but does not add a normative
ACP requirement to the spec.

## Future Work (Not Part of This Proposal)

### ACP delivery-side prime timeout

The ACP delivery-side wait today lacks a bounded prime window. The
per-RPC `RecvTimeoutError::Timeout` in `src/acp/client.rs` is a per-call
timeout, not a delivery-side prime bound. A follow-up OpenSpec would
introduce an ACP-side `acp-prime-timeout-ms` config key and a clock that
starts when input is sent and resets when the worker transitions out of
`Busy`/`Initializing`. On fire, the flush group would resolve as
`SendOutcome::Timeout` with `reason_code = "acp_turn_timeout"`.

### ACP delivery-side wedge detection

The ACP wedge model is `WorkerReadinessState::Available AND no outstanding
pending choice request`. A follow-up OpenSpec would introduce an
`acp-wedge-detection` config key, a per-target probe trait injected
through the ACP delivery path, and an outcome mapping to `SendOutcome::Failed`
with a transport-defined `reason_code` (e.g. `agent_wedged`). The
follow-up will add the active `session-relay` SHALL requirement for ACP
wedge detection (omitted from this proposal because implementation is
deferred).

## Risks / Trade-offs

- **Risk:** operators who set an aggressive prime timeout will see `Timeout`
  outcomes for legitimately slow agents (multi-minute tool calls). **Mitigation:**
  default is disabled (today's unbounded behavior). Documentation must state
  that the prime window is a fail-fast knob for unresponsive targets, not a
  normal delivery bound.
- **Risk:** wedge detection depends on the prompt regex matching. A misconfigured
  prompt regex will produce false-positive wedges. **Mitigation:** same as
  existing prompt-readiness gating — operators are responsible for configuring
  the regex; misconfiguration surfaces as observable delivery failure, not
  silent corruption. Add an operator-facing diagnostic on wedge detection
  (log line + inscription) so the failure mode is debuggable.
- **Risk:** `operator_interaction_active` shells out to tmux for every prime
  check tick. **Mitigation:** today the check already runs post-quiescence;
  adding it pre-quiescence during the prime window adds the same cost.
  Acceptable; tmux is local IPC.
- **Risk:** a perpetually-active operator interaction indefinitely blocks
  delivery with no timeout fallback. **Mitigation:** this is the correct
  operational behavior — the target is genuinely blocked and the operator
  is expected to clear the interaction. If a "fail after N minutes of
  pending operator interaction" policy is needed in the future, it can
  land as a separate config knob (e.g. `operator-interaction-timeout`)
  without conflicting with the current proposal.

## Migration Plan

1. Land OpenSpec deltas for `transport-abstraction` (three-state model) and
   `session-relay` (prime timeout and wedge detection requirements;
   MODIFIED Quiescence-Gated Delivery for transport-specific semantics;
   MODIFIED Prompt-Readiness Template Gating for wedge integration).
2. Extend `QuiescenceOptions` to carry prime timeout; surface config keys.
3. Add `DeliveryWaitError::Wedged` variant and `PaneQuiescenceProbe` trait.
4. Refactor `wait_for_quiescent_pane` to drive the three-state classifier
   through the probe trait.
5. Update `wait_error_to_outcome` to map `Wedged → SendOutcome::Failed` with
   `reason_code = "pane_wedged"` and `Timeout → SendOutcome::Timeout`.
6. Add the five-probe test surface in `tests/unit/tmux_transport.rs`.
7. Document the config keys in operator-facing docs.
8. Roll back by reverting the OpenSpec and the corresponding `src/` changes;
   no migration of persisted state is required (no schema changes).

## Open Questions

- Should the prime timeout and wedge detection be the same config key
  (interpreted by the transport) or two separate keys? **Recommendation:**
  separate keys — operators may want bounded responsiveness with no wedge
  detection (e.g. for autonomous agent flows), or wedge detection without a
  prime deadline (e.g. for human-in-the-loop flows). Keep the keys
  independent. **Locked decision in this proposal: separate keys.**
- Should the wedge outcome be surfaced as a distinct user-visible CLI/MCP
  status string (e.g. `outcome = "wedged"`) or collapsed into `failed`?
  **Recommendation:** keep collapsed into `failed` with
  `reason_code = "pane_wedged"` for v1 (Coordinator's locked decision). The
  relay worker already maps `Failed` with arbitrary reason codes; no relay
  edits required. Add a distinct string only if operators report a need.
- Does `PaneQuiescenceProbe` belong in `src/tmux/transport.rs` or
  `src/tmux/pane.rs`? **Recommendation:** `tmux/transport.rs` — the probe
  is a transport-internal seam for the wait function, not a tmux pane query
  primitive. Keeps `tmux/pane.rs` focused on tmux IPC.