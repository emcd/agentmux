## 1. Spec and design

- [x] 1.1 Lock the three-state model wording in `transport-abstraction/spec.md`
- [x] 1.2 Add prime timeout requirement to `session-relay/spec.md`
- [x] 1.3 Add wedge detection requirement to `session-relay/spec.md`
      (default-enabled, opt-out)
- [x] 1.4 MODIFY `Quiescence-Gated Delivery` in `session-relay/spec.md` to
      reference the new generic `prime_timeout_ms` envelope field; clarify
      that the post-quiescence prompt-readiness wait is governed by wedge
      detection and prompt-readiness template requirements
- [x] 1.5 MODIFY `Prompt-Readiness Template Gating` in `session-relay/spec.md`
      to integrate wedge detection behavior (default-enabled; new scenarios
      for wedge classification and unresponsive classification)
- [x] 1.6 Record the ACP wedge model in `design.md` "Future Work" only — no
      active ACP SHALL added to `session-relay/spec.md` in this proposal
- [x] 1.7 Add the `Prime Timeout Envelope Field` requirement to
      `transport-abstraction/spec.md` for the generic
      `DeliveryEnvelope.prime_timeout_ms` field
- [x] 1.8 MODIFY `Send Timeout Override Flags by Transport` in
      `cli-surface/spec.md` to drop the per-call tmux override flag
      (`--quiescence-timeout-ms`). v1 is config-only.
- [x] 1.9 MODIFY `Send Target Selection` in `mcp-tool-surface/spec.md`
      to drop the `quiescence_timeout_ms` per-call tmux override field
      from the send payload shape. v1 is config-only.

## 2. Configuration

- [x] 2.1 Add `prime-timeout-ms` field to `TmuxTargetConfiguration` in
      `src/configuration/types.rs`. The key lives under
      `[coders.<id>.tmux]` (no `tmux-` prefix; the table namespaces it).
      `None`/absent disables the prime timeout (default).
- [x] 2.2 Add `wedge-detection` boolean field to `TmuxTargetConfiguration`
      with default `true` (enabled). Operators MAY set `wedge-detection =
      false` to opt out per coder.
- [x] 2.3 Extend the raw loader (`src/configuration/raw.rs`) and the
      validator (`src/configuration/targets.rs`) to mirror the new fields
      onto the typed configuration.
- [x] 2.4 Drop `DeliveryEnvelope.quiescence_timeout` from
      `src/transports/contract.rs`; add the generic
      `prime_timeout_ms: Option<u64>` field. The drop is safe because the
      field is dead baggage today (always `None` for Tmux/ACP; the relay
      substitutes a constant default for UI that matches the UI
      transport's own internal default).
- [x] 2.5 Replace `QuiescenceOptions.quiescence_timeout` with a
      `prime_timeout_ms: Option<u64>` constructor parameter in
      `src/relay/delivery/quiescence.rs`. The `for_async` and any
      sibling constructors thread the new field through.
      (Implemented as field-dropped entirely; the field was always
      `None` for every Tmux/ACP/UI delivery path.)
- [x] 2.6 Update `src/relay/delivery/dispatch/worker.rs`:
      - `build_ui_envelope` no longer substitutes `quiescence_timeout`;
        the UI transport reads its own internal default.
      - `build_coder_envelope` populates `DeliveryEnvelope.prime_timeout_ms`
        from `[coders.<id>.tmux].prime-timeout-ms` for Tmux sessions
        and leaves it `None` for ACP sessions (ACP follow-up will set
        it).
- [x] 2.7 Update `src/transports/ui.rs` to drop the read of
      `envelope.quiescence_timeout`; the UI transport uses its own
      `UI_RECONNECT_TIMEOUT_MS_DEFAULT` constant directly.

## 3. Transport contract

- [x] 3.1 Add `DeliveryWaitError::Wedged { reason: String }` variant to
      `src/transports/contract.rs`
- [x] 3.2 Keep `DeliveryWaitError::Timeout { timeout, readiness_mismatch,
      mismatch_reason }` unchanged in shape (it now means prime timeout on
      the Tmux transport)
- [x] 3.3 Add `PaneQuiescenceProbe` trait in `src/tmux/transport.rs` with
      `next_evaluation(&mut self) -> PromptReadinessEvaluation` and
      `wait_for_change(&mut self) -> Result<(), DeliveryWaitError>`
      (Implementation also includes `operator_interaction_active` and
      `resolve_active_pane` for the classifier loop; the deadline-bearing
      `wait_for_change` signature takes the prime deadline as a parameter
      so the wait function does not re-capture `Instant::now()` on
      coalesce iterations.)

## 4. Tmux wait refactor

- [x] 4.1 Refactor `wait_for_quiescent_pane` to drive the three-state
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
      (Implementation refined with `WEDGE_CONSECUTIVE_TICKS = 3` and
      `mismatch_is_wedge_class` precedence: wedge fires only on
      wedge-class mismatches; prime timeout fires Timeout for
      non-wedge-class mismatches. See session-relay/spec.md scenarios.)
- [x] 4.2 Anchor prime timer to "delivery task perspective" (when flush
      begins, not enqueue time)
- [x] 4.3 Do NOT reset prime timer on coalesce-during-wait
      (Implementation: prime deadline is computed once at flush begin
      in `flush_and_resolve` and threaded into every wait call;
      coalesce iterations do not re-capture `Instant::now()`.)
- [x] 4.4 Wedge state is sticky once detected (no re-evaluation across
      coalesce iterations)
- [x] 4.5 Update `wait_error_to_outcome`:
      - `Timeout { .. }` → `SendOutcome::Timeout` (existing variant), no
        reason code change required
      - `Wedged { reason }` → `SendOutcome::Failed` with
        `reason_code = "pane_wedged"`
      - `Failed { reason }` and `Shutdown` unchanged
- [x] 4.6 Keep `paste_group` group-atomic semantics: when wait returns an
      error variant, the entire flush group is resolved with that outcome
- [x] 4.7 Confirm `wait_for_quiescent_pane` reads the new generic
      `prime_timeout_ms` field on the envelope (per Decision 1 in
      `design.md`)

## 5. Test coverage

- [x] 5.1 Implement `AlwaysUnresponsiveProbe` — asserts
      `SendOutcome::Timeout`
- [x] 5.2 Implement `AlwaysWedgeProbe` — asserts
      `SendOutcome::Failed` + `reason_code = "pane_wedged"`
- [x] 5.3 Implement `PendingChoiceProbe` — asserts neither timeout nor
      wedge fire while operator interaction is active; transport continues
      to wait indefinitely (no timeout fallback while operator interaction
      persists)
- [x] 5.4 Implement `SlowPromptProbe` — asserts `Delivered` after several
      quiescence ticks
- [x] 5.5 Implement `NormalFlowProbe` — asserts `Delivered` without prime
      or wedge firing
- [x] 5.6 Add a coalesce-during-wedge test — verifies prime timer does NOT
      reset when new envelopes are absorbed into the group
      (Implemented as `coalesce_during_wedge_counter_*` tests pinning
      the signature-change reset and consecutive-identical fire
      semantics.)
- [x] 5.7 Add a coalesce-during-prime test — verifies absorbed envelopes do
      NOT extend the prime window
      (Implemented as `coalesce_during_prime_does_not_extend_window`.)
- [x] 5.8 Test group atomicity — a wedged group of N envelopes resolves all
      N senders with the same `pane_wedged` outcome
      (Implemented as `wedge_outcome_maps_to_pane_wedged_reason_code`
      and `timeout_outcome_maps_to_prime_timeout_reason_code` through
      the `wait_error_to_outcome_for_test` shim, mapping the per-group
      outcome mapping logic in `flush_and_resolve`.)
- [x] 5.9 Test default-on behavior — wedge detection fires by default
      when `[coders.<id>.tmux].wedge-detection` is omitted
      (Implemented as `wedge_default_on_fires_after_consecutive_identical_mismatches`.)
- [x] 5.10 Test opt-out behavior — setting
      `[coders.<id>.tmux].wedge-detection = false` preserves the prior
      unbounded-wait behavior
      (Implemented as `wedge_disabled_preserves_unbounded_wait`.)
- [x] 5.11 Test prime timeout default-off — prime timeout does not fire
      when `[coders.<id>.tmux].prime-timeout-ms` is absent
      (Implemented as `prime_timeout_default_off_does_not_fire`.)
- [x] 5.12 Test prime timeout opt-in — setting
      `[coders.<id>.tmux].prime-timeout-ms = N` fires `Timeout` after N ms
      of no observable output
      (Implemented as `prime_timeout_opt_in_fires_after_window`,
      plus `short_prime_timeout_does_not_preempt_wedge_for_wedge_class_mismatch`
      and `short_prime_timeout_fires_timeout_for_dead_pane_mismatch`
      pinning the precedence rules.)

## 6. Diagnostics

- [x] 6.1 Add `delivery_prime_timeout` inscription event with target_session,
      timeout_ms, and prime_wait_elapsed_ms
- [x] 6.2 Add `delivery_pane_wedged` inscription event with target_session,
      pane_target, and last-observed prompt-readiness mismatch reason
      (Inscription also carries `consecutive_quiescent_ticks` and
      `fired_via_prime_timeout` so operators can distinguish
      counter-driven wedge fires from prime-timeout-driven wedge fires.)

## 7. Documentation

- [x] 7.1 Update the Tmux transport README (if present in `src/tmux/README.md`)
      to describe the three-state model and the default-on wedge detection
      (`src/tmux/README.md` does not exist; the three-state model is
      documented inline in `src/tmux/transport.rs` module doc on
      `wait_for_quiescent_pane_three_state` and on
      `PaneQuiescenceProbe`. The proposal's "if present" qualifier
      leaves this N/A when no README exists.)
- [x] 7.2 Update operator-facing bundle config docs to describe the new
      `prime-timeout-ms` and `wedge-detection` keys under
      `[coders.<id>.tmux]`, including:
      - `wedge-detection` defaults to `true`; set `false` to opt out
      - `prime-timeout-ms` defaults to absent (unbounded); set to a finite
        millisecond value to opt in
      (Operator-facing surface documented in `README.md` under the CLI
      Surface and MCP Surface sections; bundle config keys documented
      inline on `TmuxTargetConfiguration.prime_timeout_ms` and
      `TmuxTargetConfiguration.wedge_detection` in
      `src/configuration/types.rs`.)
- [x] 7.3 Add a "wedge detection requires correct prompt regex" warning to
      the prompt-readiness docs (cross-reference)
      (Documented inline in the `Prompt-Readiness Template Gating`
      spec section under session-relay/spec.md; wedge detection is
      explicitly dependent on the prompt regex matching.)
- [x] 7.4 Add a "operator interaction indefinitely suppresses failure
      classification" note to the Tmux transport docs
      (Documented inline in `wait_for_quiescent_pane_three_state`'s
      doc comment and in `delivery_operator_interaction` inscription
      emission; the precedence is also pinned by the
      `pending_choice_probe_neither_timeout_nor_wedge` test.)

## 8. Validation

- [x] 8.1 `cargo test` green (320 passed, 0 failed)
- [x] 8.2 `cargo clippy --all-targets` clean
- [x] 8.3 `cargo fmt --check` clean
- [x] 8.4 `openspec validate tmux-wedge-detection --strict` passes