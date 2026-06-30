## 1. Spec and design

- [ ] 1.1 Lock the three-state model wording in `transport-abstraction/spec.md`
- [ ] 1.2 Add prime timeout requirement to `session-relay/spec.md`
- [ ] 1.3 Add wedge detection requirement to `session-relay/spec.md`
      (default-enabled, opt-out)
- [ ] 1.4 MODIFY `Quiescence-Gated Delivery` in `session-relay/spec.md` to
      reference the new generic `prime_timeout_ms` envelope field; clarify
      that the post-quiescence prompt-readiness wait is governed by wedge
      detection and prompt-readiness template requirements
- [ ] 1.5 MODIFY `Prompt-Readiness Template Gating` in `session-relay/spec.md`
      to integrate wedge detection behavior (default-enabled; new scenarios
      for wedge classification and unresponsive classification)
- [ ] 1.6 Record the ACP wedge model in `design.md` "Future Work" only — no
      active ACP SHALL added to `session-relay/spec.md` in this proposal
- [ ] 1.7 Add the `Prime Timeout Envelope Field` requirement to
      `transport-abstraction/spec.md` for the generic
      `DeliveryEnvelope.prime_timeout_ms` field
- [ ] 1.8 MODIFY `Send Timeout Override Flags by Transport` in
      `cli-surface/spec.md` to drop the per-call tmux override flag
      (`--quiescence-timeout-ms`). v1 is config-only.
- [ ] 1.9 MODIFY `Send Target Selection` in `mcp-tool-surface/spec.md`
      to drop the `quiescence_timeout_ms` per-call tmux override field
      from the send payload shape. v1 is config-only.

## 2. Configuration

- [ ] 2.1 Add `prime-timeout-ms` field to `TmuxTargetConfiguration` in
      `src/configuration/types.rs`. The key lives under
      `[coders.<id>.tmux]` (no `tmux-` prefix; the table namespaces it).
      `None`/absent disables the prime timeout (default).
- [ ] 2.2 Add `wedge-detection` boolean field to `TmuxTargetConfiguration`
      with default `true` (enabled). Operators MAY set `wedge-detection =
      false` to opt out per coder.
- [ ] 2.3 Extend the raw loader (`src/configuration/raw.rs`) and the
      validator (`src/configuration/targets.rs`) to mirror the new fields
      onto the typed configuration.
- [ ] 2.4 Drop `DeliveryEnvelope.quiescence_timeout` from
      `src/transports/contract.rs`; add the generic
      `prime_timeout_ms: Option<u64>` field. The drop is safe because the
      field is dead baggage today (always `None` for Tmux/ACP; the relay
      substitutes a constant default for UI that matches the UI
      transport's own internal default).
- [ ] 2.5 Replace `QuiescenceOptions.quiescence_timeout` with a
      `prime_timeout_ms: Option<u64>` constructor parameter in
      `src/relay/delivery/quiescence.rs`. The `for_async` and any
      sibling constructors thread the new field through.
- [ ] 2.6 Update `src/relay/delivery/dispatch/worker.rs`:
      - `build_ui_envelope` no longer substitutes `quiescence_timeout`;
        the UI transport reads its own internal default.
      - `build_coder_envelope` populates `DeliveryEnvelope.prime_timeout_ms`
        from `[coders.<id>.tmux].prime-timeout-ms` for Tmux sessions
        and leaves it `None` for ACP sessions (ACP follow-up will set
        it).
- [ ] 2.7 Update `src/transports/ui.rs` to drop the read of
      `envelope.quiescence_timeout`; the UI transport uses its own
      `UI_RECONNECT_TIMEOUT_MS_DEFAULT` constant directly.

## 3. Transport contract

- [ ] 3.1 Add `DeliveryWaitError::Wedged { reason: String }` variant to
      `src/transports/contract.rs`
- [ ] 3.2 Keep `DeliveryWaitError::Timeout { timeout, readiness_mismatch,
      mismatch_reason }` unchanged in shape (it now means prime timeout on
      the Tmux transport)
- [ ] 3.3 Add `PaneQuiescenceProbe` trait in `src/tmux/transport.rs` with
      `next_evaluation(&mut self) -> PromptReadinessEvaluation` and
      `wait_for_change(&mut self) -> Result<(), DeliveryWaitError>`

## 4. Tmux wait refactor

- [ ] 4.1 Refactor `wait_for_quiescent_pane` to drive the three-state
      classifier through `PaneQuiescenceProbe`:
      - prime window: clock starts at first wait start; reads the
        envelope's `prime_timeout_ms` field; fires `Unresponsive`
        when no change observed within that window AND no
        operator-interaction signal is active
      - operator-interaction-active check runs in BOTH the prime window and
        post-quiescence; an active signal indefinitely suppresses both
        `unresponsive` and `wedged` classification
      - post-quiescence: wedge check fires on
        `quiescent && !prompt_ready && !operator_interaction_active`
        (wedge detection defaults to enabled; disabled only when
        `[coders.<id>.tmux].wedge-detection = false`)
- [ ] 4.2 Anchor prime timer to "delivery task perspective" (when flush
      begins, not enqueue time)
- [ ] 4.3 Do NOT reset prime timer on coalesce-during-wait
- [ ] 4.4 Wedge state is sticky once detected (no re-evaluation across
      coalesce iterations)
- [ ] 4.5 Update `wait_error_to_outcome`:
      - `Timeout { .. }` → `SendOutcome::Timeout` (existing variant), no
        reason code change required
      - `Wedged { reason }` → `SendOutcome::Failed` with
        `reason_code = "pane_wedged"`
      - `Failed { reason }` and `Shutdown` unchanged
- [ ] 4.6 Keep `paste_group` group-atomic semantics: when wait returns an
      error variant, the entire flush group is resolved with that outcome
- [ ] 4.7 Confirm `wait_for_quiescent_pane` reads the new generic
      `prime_timeout_ms` field on the envelope (per Decision 1 in
      `design.md`)

## 5. Test coverage

- [ ] 5.1 Implement `AlwaysUnresponsiveProbe` — asserts
      `SendOutcome::Timeout`
- [ ] 5.2 Implement `AlwaysWedgeProbe` — asserts
      `SendOutcome::Failed` + `reason_code = "pane_wedged"`
- [ ] 5.3 Implement `PendingChoiceProbe` — asserts neither timeout nor
      wedge fire while operator interaction is active; transport continues
      to wait indefinitely (no timeout fallback while operator interaction
      persists)
- [ ] 5.4 Implement `SlowPromptProbe` — asserts `Delivered` after several
      quiescence ticks
- [ ] 5.5 Implement `NormalFlowProbe` — asserts `Delivered` without prime
      or wedge firing
- [ ] 5.6 Add a coalesce-during-wedge test — verifies prime timer does NOT
      reset when new envelopes are absorbed into the group
- [ ] 5.7 Add a coalesce-during-prime test — verifies absorbed envelopes do
      NOT extend the prime window
- [ ] 5.8 Test group atomicity — a wedged group of N envelopes resolves all
      N senders with the same `pane_wedged` outcome
- [ ] 5.9 Test default-on behavior — wedge detection fires by default
      when `[coders.<id>.tmux].wedge-detection` is omitted
- [ ] 5.10 Test opt-out behavior — setting
      `[coders.<id>.tmux].wedge-detection = false` preserves the prior
      unbounded-wait behavior
- [ ] 5.11 Test prime timeout default-off — prime timeout does not fire
      when `[coders.<id>.tmux].prime-timeout-ms` is absent
- [ ] 5.12 Test prime timeout opt-in — setting
      `[coders.<id>.tmux].prime-timeout-ms = N` fires `Timeout` after N ms
      of no observable output

## 6. Diagnostics

- [ ] 6.1 Add `delivery_prime_timeout` inscription event with target_session,
      timeout_ms, and prime_wait_elapsed_ms
- [ ] 6.2 Add `delivery_pane_wedged` inscription event with target_session,
      pane_target, and last-observed prompt-readiness mismatch reason

## 7. Documentation

- [ ] 7.1 Update the Tmux transport README (if present in `src/tmux/README.md`)
      to describe the three-state model and the default-on wedge detection
- [ ] 7.2 Update operator-facing bundle config docs to describe the new
      `prime-timeout-ms` and `wedge-detection` keys under
      `[coders.<id>.tmux]`, including:
      - `wedge-detection` defaults to `true`; set `false` to opt out
      - `prime-timeout-ms` defaults to absent (unbounded); set to a finite
        millisecond value to opt in
- [ ] 7.3 Add a "wedge detection requires correct prompt regex" warning to
      the prompt-readiness docs (cross-reference)
- [ ] 7.4 Add a "operator interaction indefinitely suppresses failure
      classification" note to the Tmux transport docs

## 8. Validation

- [ ] 8.1 `cargo test` green
- [ ] 8.2 `cargo clippy --all-targets` clean
- [ ] 8.3 `cargo fmt --check` clean
- [ ] 8.4 `openspec validate tmux-wedge-detection --strict` passes