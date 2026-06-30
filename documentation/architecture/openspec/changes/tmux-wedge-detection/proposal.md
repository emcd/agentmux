# Change: Tmux wedge detection and prime timeout

## Why

Today, `TmuxTransport::wait_for_quiescent_pane` runs unbounded: it waits for pane
quiescence forever, bounded only by relay shutdown. Two target failure modes are
not distinguishable from healthy behavior:

- **Unresponsive target** — during the quiescence wait for a flush group, the
  pane produces no output at all within any window and no operator-interaction
  signal is active. The transport waits indefinitely.
- **Wedged target** — output settles at a non-prompt state with no pending
  operator interaction. The transport continues to wait indefinitely.

Operators cannot distinguish "agent is thinking for a long time" from "the pane
is hung on a tool-approval dialog the prompt regex does not match" or "the
target process died without closing." A bounded prime window and a wedge
detector resolve both failure modes with distinct outcomes.

## What Changes

- Introduce a three-state delivery state machine for the Tmux transport:
  `unresponsive` (no output within prime window), `running` (output flowing or
  settled at prompt), `wedged` (output settled, not prompt-ready, no pending
  choices).
- Add a config-surfaced prime timeout knob for Tmux delivery, under the
  per-coder `[coders.<id>.tmux]` table as the `prime-timeout-ms` TOML key
  (no `tmux-` prefix — the table itself namespaces the key). When the prime
  window elapses during the quiescence wait with no observable output and no
  operator interaction, the flush group resolves as `SendOutcome::Timeout`
  (existing variant). The prime timeout is configured per-coder under
  `[coders.<id>.tmux]` and is opt-in; `None` (or absent) preserves
  today's unbounded behavior. v1 does NOT expose a per-call
  `agentmux send` / MCP `send` override for the tmux prime timeout —
  it is config-only.
- Add a config-surfaced wedge detection knob for Tmux delivery, under the
  per-coder `[coders.<id>.tmux]` table as the `wedge-detection` TOML key.
  Wedge detection defaults to **enabled** (opt-out) because the cost of a
  silently-wedged pane is high (relay queue grows, future deliveries fail)
  and the cost of a false-positive wedge is recoverable (operator restarts
  the target). Operators MAY disable wedge detection explicitly by setting
  the key to `false`. When enabled and the pane becomes quiescent + stays
  not-prompt-ready + no operator interaction, the flush group resolves as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`.
- Active operator-interaction signals (copy-mode or key-table for tmux)
  indefinitely suppress both `unresponsive` and `wedged` classification.
  Prime timeout does not fire while operator interaction is active.
- Replace the existing `DeliveryEnvelope.quiescence_timeout: Option<Duration>`
  field with a generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>`
  field. The `quiescence_timeout` field is dead baggage: today it is
  always `None` for every Tmux and ACP path (the relay's
  `QuiescenceOptions::for_async` hardwires it), and for the UI path the
  relay substitutes a constant default that matches the UI transport's
  own internal default. The new generic `prime_timeout_ms` field carries
  a per-coder prime bound from the relay to any transport that performs
  a prime wait (Tmux today; the ACP follow-up will use the same field).
- Modify the existing `Quiescence-Gated Delivery` requirement to clarify
  that the Tmux transport reads `DeliveryEnvelope.prime_timeout_ms` to
  bound the prime window (not the post-quiescence wait), and that the
  post-quiescence prompt-readiness wait is governed by wedge detection
  and prompt-readiness template requirements.
- Refactor `TmuxTransport::wait_for_quiescent_pane` to route the existing
  quiescence/poll machinery through the three-state classifier. The
  `DeliveryWaitError::Timeout` mapping in `wait_error_to_outcome` switches
  from `SendOutcome::Failed` + `reason_code = "quiescence_timeout"` to
  `SendOutcome::Timeout`; a new `DeliveryWaitError::Wedged` variant maps to
  `SendOutcome::Failed` + `reason_code = "pane_wedged"`.
- Introduce a `PaneQuiescenceProbe` trait in the Tmux transport module so
  tests can inject deterministic probe implementations covering the five
  behavior classes: unresponsive, wedged, pending-choice, slow-prompt, normal.

## Amendment history

This proposal was originally merged on master (commit `dfea776`, merge
`3bca97b`). The feedback items below were raised post-merge; this
proposal was amended in place to incorporate them rather than starting a
follow-up change so the implementation lands against a single consistent
spec.

1. **Default wedge detection ON** (opt-out rather than opt-in). The original
   draft marked both knobs opt-in. Wedge detection is more important to
   have on than off — the cost of a silently-wedged pane (queue growth,
   delivery backlog) is higher than the cost of a false-positive wedge
   (operator restarts the target). Prime timeout remains opt-in because it
   can produce false positives for legitimately slow agents.
2. **TOML placement and key naming.** The original draft used transport-
   prefixed keys (`tmux-prime-timeout-ms`, `tmux-wedge-detection`) under
   per-bundle or per-session scope. After review, the keys live under
   the per-coder `[coders.<id>.tmux]` table, where they take the table's
   implicit namespace and drop the `tmux-` prefix:
   `prime-timeout-ms` and `wedge-detection`. v1 is per-coder only; no
   per-bundle or per-session scope is exposed for these keys.
3. **Drop `DeliveryEnvelope.quiescence_timeout` rather than repurposing
   it.** The original draft repurposed the existing `quiescence_timeout`
   field on `DeliveryEnvelope` to mean "prime timeout on Tmux." This was
   not acceptable — wire fields must keep their meaning across
   implementations to avoid ambiguity for downstream readers. The
   amendment drops the field entirely (it is dead baggage today:
   always `None` for Tmux/ACP; the relay substitutes a constant default
   for UI that matches the UI transport's own internal default) and
   introduces a generic `prime_timeout_ms: Option<u64>` field instead.
   The new field is generic across transports, so the same shape serves
   the ACP follow-up without per-transport field proliferation on the
   envelope.
4. **Generic prime timeout field, not transport-specific.** The
   first-pass amendment introduced a `tmux_prime_timeout_ms: Option<u64>`
   field with a `tmux_` prefix. Coordinator flagged this as inconsistent
   with the transport-decoupling direction — the relay should not
   know about per-transport timeout fields. The amendment renames the
   field to a generic `prime_timeout_ms: Option<u64>` that the relay
   populates from the per-coder config and that any prime-wait
   transport (Tmux today, ACP in the follow-up) MAY consume.

## Impact

- Affected specs: `transport-abstraction`, `session-relay`.
- Affected code:
  - `src/transports/contract.rs` — remove `DeliveryEnvelope.quiescence_timeout`
    field; add `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field.
  - `src/tmux/transport.rs` — extend `wait_for_quiescent_pane` to consume
    the new `prime_timeout_ms` envelope field, add `PaneQuiescenceProbe`
    trait, split `DeliveryWaitError`, update `wait_error_to_outcome`.
  - `src/tmux/pane.rs` — no changes expected; the probe trait is a
    transport-internal seam in `tmux/transport.rs`.
  - `src/transports/ui.rs` — drop the read of
    `envelope.quiescence_timeout`; the UI transport uses its own internal
    `UI_RECONNECT_TIMEOUT_MS_DEFAULT` constant directly.
  - `src/relay/delivery/quiescence.rs` — replace `quiescence_timeout`
    field on `QuiescenceOptions` with a `prime_timeout_ms` constructor
    parameter (or add a sibling constructor that threads the new field).
  - `src/relay/delivery/dispatch/worker.rs` — update
    `build_ui_envelope` (drop the `quiescence_timeout` substitution) and
    `build_coder_envelope` (populate `prime_timeout_ms` from
    `[coders.<id>.tmux].prime-timeout-ms`).
  - `src/configuration/types.rs` — add `prime_timeout_ms` and
    `wedge_detection` fields to `TmuxTargetConfiguration`. The keys live
    under `[coders.<id>.tmux]` and do NOT take a `tmux-` prefix.
  - `src/configuration/raw.rs` and `targets.rs` — mirror the new fields
    through the raw loader and validator.
- Backwards-compatible for prime timeout (default `None`, preserves
  today's unbounded behavior). Wedge detection default is `true` — this
  is a behavior change for existing Tmux deployments: today a wedged pane
  is silently waited on indefinitely; with this proposal the wedge fires
  by default. Operators MAY set `wedge-detection = false` to preserve the
  prior behavior. The drop of `quiescence_timeout` is a wire-format
  change for the relay envelope; only the relay, the UI transport, and
  the Tmux transport touch it today, and all three are updated in this
  proposal.
- Out of scope (deferred to a follow-up OpenSpec): ACP delivery-side prime
  timeout and wedge detection implementation. The ACP wedge model is
  recorded in `design.md` as future-work context only — no active
  `session-relay` SHALL requirement for ACP is added by this proposal.