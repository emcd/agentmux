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
- Config-surfaced Tmux prime timeout (`prime-timeout-ms` under
  `[coders.<id>.tmux]`) — opt-in, defaults to disabled, preserves today's
  unbounded behavior.
- Config-surfaced Tmux wedge detection (`wedge-detection` under
  `[coders.<id>.tmux]`) — defaults to **enabled** (opt-out) because the
  cost of a silently-wedged pane is high.
- Distinct operator-visible outcomes for the two failure modes (`Timeout`
  vs. `Failed` with `reason_code = "pane_wedged"`).
- Deterministic, fast test surface via injected `PaneQuiescenceProbe`
  trait.
- Group-level atomicity: a wedged or unresponsive flush group fails as a
  whole. No per-message deadlines.
- Active operator-interaction signals (tmux copy-mode or key-table)
  suppress both `unresponsive` and `wedged` classification while active.
  Prime timeout does not fire while operator interaction is active.
- New generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field for
  any transport that performs a prime wait. The relay populates this
  from per-coder config without knowing which transport will consume it.
- Drop the existing `DeliveryEnvelope.quiescence_timeout` field (dead
  baggage — always `None` for Tmux/ACP; the UI transport already has its
  own matching internal default).

## Non-Goals

- No ACP delivery-side prime timeout or wedge detection implementation.
- No per-call operator override (config-only for v1; per-call override is
  a backward-compatible follow-up if/when operators need it).
- No new `SendOutcome` variant. Both outcomes reuse existing variants.
- No relay-side changes beyond the new envelope field and the dropped
  old one — the relay worker already maps `Timeout` and `Failed` with
  arbitrary `reason_code` values; no `async_worker.rs` edits are required.

## Decisions

### Decision 1: Drop `DeliveryEnvelope.quiescence_timeout` and add a generic `prime_timeout_ms`

The original draft repurposed `DeliveryEnvelope.quiescence_timeout` to mean
"prime timeout on Tmux." A subsequent amendment introduced a transport-
specific `tmux_prime_timeout_ms: Option<u64>` field. Coordinator flagged
both approaches as problematic:

- Repurposing a wire field without renaming creates ambiguity for
  downstream readers (logs, MCP/CLI consumers, future debugging tools).
- A transport-prefixed field on a generic envelope increases the
  transport-relay coupling the decoupling arc was designed to remove.

The current amendment addresses both concerns with a single change:
**drop `quiescence_timeout` entirely and add a generic `prime_timeout_ms:
Option<u64>` field**.

The `quiescence_timeout` field is dead baggage today. The relay's
`QuiescenceOptions::for_async` hardwires `quiescence_timeout: None`
(`src/relay/delivery/quiescence.rs:36`), so every Tmux and ACP delivery
sees `None`. The UI path is the only consumer: `build_ui_envelope`
substitutes `QUIESCENCE_TIMEOUT_MS_DEFAULT = 30_000` for the field, and
the UI transport's `UI_RECONNECT_TIMEOUT_MS_DEFAULT = 30_000` matches
that constant exactly. Dropping the field is therefore a no-op for UI
behavior — the transport uses its own constant directly.

The new `prime_timeout_ms` field is generic: any prime-wait transport
(Tmux today; ACP in the follow-up) MAY consume it. The relay populates
it from per-coder config without knowing the transport. This keeps the
envelope transport-neutral while still threading bounded-wait knobs
across the relay boundary.

### Decision 2: Wedge detection defaults to enabled (opt-out)

Wedge detection is more important to have on than off:

- **Cost of a silently-wedged pane (today's behavior):** delivery queue
  grows without bound, future deliveries fail or back up, operator sees
  no signal that something is wrong until they manually inspect the
  target. This is a high-impact silent failure.
- **Cost of a false-positive wedge (with default-on detection):** the
  flush group fails with `Failed` + `reason_code = "pane_wedged"`,
  operator restarts the target, future deliveries proceed normally.
  Recoverable in minutes.

The default-on choice matches operator expectations for safety: a
fail-fast detection is preferred over silent queue growth.

Prime timeout remains opt-in because the cost asymmetry is reversed: a
false-positive prime timeout on a slow but legitimate agent produces a
hard `Timeout` outcome for an in-progress turn, which is harder to
recover from than a wedge. Operators MUST explicitly opt in to prime
timeout.

### Decision 3: Config keys live under `[coders.<id>.tmux]` without `tmux-` prefix

The bundle configuration places each coder's Tmux target under a
`[coders.<id>.tmux]` table (`src/configuration/types.rs:143`,
`TmuxTargetConfiguration`). The table itself namespaces the keys. Adding
a `tmux-` prefix on top of the table prefix is redundant and conflicts
with the project's TOML naming convention (noun-prefixed only when the
broader scope is not already transport-specific).

The new keys:

- `prime-timeout-ms` — integer milliseconds, `None` disables the bound.
- `wedge-detection` — boolean, defaults to `true` (enabled).

The keys live in `TmuxTargetConfiguration` alongside the existing
`start_command` and `prompt_readiness` fields. The validator
(`src/configuration/targets.rs:199` `validate_tmux_target`) is the
single place where defaults and validation live for Tmux target
fields.

### Decision 4: `operator_interaction_active` indefinitely suppresses both classifiers

Today `operator_interaction_active` is checked only after the pane
becomes quiescent. For the three-state model we check it during the
prime window AS WELL. While `operator_interaction_active` reports an
active copy-mode or key-table for the target session, both
`unresponsive` and `wedged` classification are indefinitely suppressed
— the transport continues to wait and the prime timer does NOT fire.

This is the correct operational policy: an active operator interaction
means the user (or an external decision) is legitimately in progress.
Firing `unresponsive` while the user is in copy-mode would falsely
classify the target as failed; firing `wedged` while a `choose`
request is pending (classic Claude Code tool-approval flow) would
falsely classify it as wedged.

### Decision 5: Group atomicity on failure modes

When prime timeout fires, the entire flush group fails as `Timeout`.
When wedge fires, the entire flush group fails as `Failed` with
`reason_code = "pane_wedged"`. This matches `paste_group`'s group-atomic
semantics. Per-message deadlines are explicitly out of scope.

### Decision 6: Probe trait for testability

Inject a `PaneQuiescenceProbe` trait into the Tmux wait path so tests
can drive deterministic probe results. The trait returns the next
`PromptReadinessEvaluation` on demand. Tests cover at minimum:

- `AlwaysUnresponsiveProbe` — never produces output. Asserts `Timeout`.
- `AlwaysWedgeProbe` — immediately quiesces at a non-prompt state.
  Asserts `Failed` + `reason_code = "pane_wedged"`.
- `PendingChoiceProbe` — quiesces with `operator_interaction_active =
  Some(_)`. Asserts neither timeout nor wedge; asserts the prime timer
  does NOT fire while operator interaction is active (transport
  continues to wait indefinitely).
- `SlowPromptProbe` — quiesces at a prompt state after several ticks.
  Asserts `Delivered`.
- `NormalFlowProbe` — produces output then settles at a prompt. Asserts
  `Delivered` without prime or wedge firing.

The probe trait lives in `src/tmux/transport.rs` (transport-internal
seam, not part of the `Transport` contract in `src/transports/contract.rs`).

### Decision 7: ACP wedge model is future-work context only

The ACP wedge model would be `WorkerReadinessState::Available AND no
outstanding pending choice request` (analogous to tmux's "settled +
not prompt-ready + no `operator_interaction_active`"). The
`pending_choice_outcome` field on ACP transport state
(`src/acp/permission.rs`) carries the request signal.

The ACP wedge model is **not** added as an active `session-relay` SHALL
requirement in this proposal. The current proposal is Tmux-only for v1.
The ACP follow-up OpenSpec will land the ACP wedge and prime-timeout
requirements; this proposal captures the model here for design
continuity and to signal the follow-up's contract scope, but does not
add a normative ACP requirement to the spec.

When the ACP follow-up lands, it will consume the same generic
`DeliveryEnvelope.prime_timeout_ms` field introduced here, populated
from `[coders.<id>.acp].prime-timeout-ms` (or a similar per-coder key
under the ACP table).

## Future Work (Not Part of This Proposal)

### ACP delivery-side prime timeout and wedge detection

The ACP delivery-side wait today lacks a bounded prime window. The
per-RPC `RecvTimeoutError::Timeout` in `src/acp/client.rs` is a per-call
timeout, not a delivery-side prime bound. A follow-up OpenSpec would
introduce an ACP-side `prime-timeout-ms` config key under
`[coders.<id>.acp]` and a clock that starts when input is sent and
resets when the worker transitions out of `Busy`/`Initializing`. On
fire, the flush group would resolve as `SendOutcome::Timeout` with
`reason_code = "acp_turn_timeout"`.

The ACP wedge model is `WorkerReadinessState::Available AND no outstanding
pending choice request`. The follow-up would introduce an
`acp-wedge-detection` config key under `[coders.<id>.acp]`, a per-target
probe trait injected through the ACP delivery path, and an outcome
mapping to `SendOutcome::Failed` with a transport-defined `reason_code`
(e.g. `agent_wedged`).

Both ACP features will consume the same generic
`DeliveryEnvelope.prime_timeout_ms` field introduced in this proposal,
keeping the envelope transport-neutral.

## Risks / Trade-offs

- **Risk:** default-on wedge detection produces false positives for
  legitimately slow agents whose prompt regex does not match (e.g.
  Claude Code idle-state screens that vary between versions).
  **Mitigation:** operators MAY set `wedge-detection = false` to opt out
  per coder. Add an operator-facing diagnostic on wedge detection (log
  line + inscription) so the failure mode is debuggable.
- **Risk:** operators who set an aggressive prime timeout will see
  `Timeout` outcomes for legitimately slow agents (multi-minute tool
  calls). **Mitigation:** default is disabled (today's unbounded
  behavior). Documentation must state that the prime window is a
  fail-fast knob for unresponsive targets, not a normal delivery bound.
- **Risk:** wedge detection depends on the prompt regex matching. A
  misconfigured prompt regex will produce false-positive wedges.
  **Mitigation:** same as existing prompt-readiness gating — operators
  are responsible for configuring the regex; misconfiguration surfaces
  as observable delivery failure, not silent corruption. The default-on
  choice means this risk is higher than under opt-in, which is why
  `wedge-detection = false` is the explicit opt-out path.
- **Risk:** dropping `DeliveryEnvelope.quiescence_timeout` is a wire-
  format change for the relay envelope. **Mitigation:** the field is
  dead baggage (see Decision 1); no production consumer reads the field
  with a meaningful value today. The drop simplifies the envelope shape.
- **Risk:** a perpetually-active operator interaction indefinitely
  blocks delivery with no timeout fallback. **Mitigation:** this is the
  correct operational behavior — the target is genuinely blocked and
  the operator is expected to clear the interaction. If a "fail after
  N minutes of pending operator interaction" policy is needed in the
  future, it can land as a separate config knob (e.g.
  `operator-interaction-timeout`) without conflicting with the current
  proposal.

## Migration Plan

1. Land OpenSpec deltas for `transport-abstraction` (three-state model
   with default-on wedge detection; generic `prime_timeout_ms` envelope
   field), `session-relay` (prime timeout under
   `[coders.<id>.tmux].prime-timeout-ms`, wedge detection under
   `[coders.<id>.tmux].wedge-detection`; MODIFIED Quiescence-Gated
   Delivery for transport-specific semantics; MODIFIED Prompt-Readiness
   Template Gating for wedge integration), `cli-surface` (MODIFIED Send
   Timeout Override Flags by Transport to drop the per-call tmux
   override), and `mcp-tool-surface` (MODIFIED Send Target Selection to
   drop the `quiescence_timeout_ms` per-call tmux override field).
2. Drop `DeliveryEnvelope.quiescence_timeout` from
   `src/transports/contract.rs`; add `prime_timeout_ms: Option<u64>`
   field.
3. Update `src/transports/ui.rs` to drop the read of
   `envelope.quiescence_timeout` and use its own
   `UI_RECONNECT_TIMEOUT_MS_DEFAULT` constant directly.
4. Update `src/relay/delivery/quiescence.rs` to remove
   `QuiescenceOptions.quiescence_timeout`; the constructor now takes
   `prime_timeout_ms: Option<u64>` directly.
5. Update `src/relay/delivery/dispatch/worker.rs` `build_ui_envelope`
   (no more `quiescence_timeout` substitution) and `build_coder_envelope`
   (populate `prime_timeout_ms` from
   `[coders.<id>.tmux].prime-timeout-ms`).
6. Add `prime_timeout_ms` and `wedge_detection` fields to
   `TmuxTargetConfiguration`; default `wedge_detection = true`.
7. Mirror the new fields through the raw loader and validator.
8. Add `DeliveryWaitError::Wedged` variant and `PaneQuiescenceProbe`
   trait in `src/tmux/transport.rs`.
9. Refactor `wait_for_quiescent_pane` to drive the three-state
   classifier through the probe trait, reading the new
   `prime_timeout_ms` envelope field.
10. Update `wait_error_to_outcome` to map `Wedged → SendOutcome::Failed`
    with `reason_code = "pane_wedged"` and `Timeout → SendOutcome::Timeout`.
11. Add the five-probe test surface in `tests/unit/tmux_transport.rs`.
12. Document the new keys, defaults, and the
    `wedge-detection = false` opt-out path in operator-facing docs.
13. Confirm `agentmux send` rejects `--quiescence-timeout-ms` as an
    unknown flag (the flag is removed from the spec; existing
    invocations should fail at the CLI parser). The MCP `send` payload
    SHALL reject `quiescence_timeout_ms` as an unknown field.

## Open Questions

- Should the prime timeout and wedge detection be the same config key
  (interpreted by the transport) or two separate keys? **Recommendation:**
  separate keys — operators may want bounded responsiveness with no
  wedge detection (e.g. for autonomous agent flows with known
  non-prompt idle states), or wedge detection without a prime deadline
  (e.g. for human-in-the-loop flows). Keep the keys independent.
  **Locked decision in this proposal: separate keys.**
- Should the wedge outcome be surfaced as a distinct user-visible
  CLI/MCP status string (e.g. `outcome = "wedged"`) or collapsed into
  `failed`? **Recommendation:** keep collapsed into `failed` with
  `reason_code = "pane_wedged"` for v1 (Coordinator's locked decision).
  The relay worker already maps `Failed` with arbitrary reason codes;
  no relay edits required. Add a distinct string only if operators
  report a need.
- Does `PaneQuiescenceProbe` belong in `src/tmux/transport.rs` or
  `src/tmux/pane.rs`? **Recommendation:** `tmux/transport.rs` — the
  probe is a transport-internal seam for the wait function, not a tmux
  pane query primitive. Keeps `tmux/pane.rs` focused on tmux IPC.
- What is the right default for `prime-timeout-ms` if wedge detection
  is disabled? **Recommendation:** no change. Both knobs remain
  independent; disabling wedge does not enable prime.