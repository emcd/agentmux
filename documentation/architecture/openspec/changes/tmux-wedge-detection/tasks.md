## 1. Spec and design

- [ ] 1.1 Lock the three-state model wording in `transport-abstraction/spec.md`
- [ ] 1.2 Add prime timeout requirement to `session-relay/spec.md`
- [ ] 1.3 Add wedge detection requirement to `session-relay/spec.md`
- [ ] 1.4 MODIFY `Quiescence-Gated Delivery` in `session-relay/spec.md` to
      clarify transport-specific semantics (Tmux bounds the prime window;
      other transports MAY retain the existing "bound on quiescence wait"
      semantics)
- [ ] 1.5 MODIFY `Prompt-Readiness Template Gating` in `session-relay/spec.md`
      to integrate wedge detection behavior (3 new scenarios; existing
      scenarios preserved)
- [ ] 1.6 Record the ACP wedge model in `design.md` "Future Work" only — no
      active ACP SHALL added to `session-relay/spec.md` in this proposal

## 2. Configuration

- [ ] 2.1 Add `tmux-prime-timeout-ms` config key (per-bundle, per-session) —
      disabled by default, preserving unbounded behavior
- [ ] 2.2 Add `tmux-wedge-detection` config key (per-bundle, per-session) —
      disabled by default
- [ ] 2.3 Extend `QuiescenceOptions` in `src/relay/delivery/quiescence.rs` to
      carry prime timeout (or add a sibling constructor)
- [ ] 2.4 Populate the new fields onto `DeliveryEnvelope.quiescence_timeout`
      (reused as prime timeout on the Tmux side) and a new envelope field for
      wedge detection if config-only is insufficient — confirm with the field
      reuse design decision before adding a new field
- [ ] 2.5 Follow project TOML key conventions: hyphenated names
      (`tmux-prime-timeout-ms`, `tmux-wedge-detection`), transport-prefixed
      grouping consistent with adjacent transport-specific keys (the
      `prompt-regex` / `turn-timeout-ms` keys are noun-only because they
      live under a per-member or per-coder scope; transport keys take a
      `tmux-` / `acp-` prefix to disambiguate when multiple transport
      types coexist in a bundle)

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
      - prime window: clock starts at first wait start; fires `Unresponsive`
        when no change observed within `quiescence_timeout` AND no
        operator-interaction signal is active
      - operator-interaction-active check runs in BOTH the prime window and
        post-quiescence; an active signal indefinitely suppresses both
        `unresponsive` and `wedged` classification
      - post-quiescence: wedge check fires on
        `quiescent && !prompt_ready && !operator_interaction_active`
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
- [ ] 5.9 Test config-default behavior — both new keys default to
      disabled and preserve today's unbounded behavior

## 6. Diagnostics

- [ ] 6.1 Add `delivery_prime_timeout` inscription event with target_session,
      timeout_ms, and prime_wait_elapsed_ms
- [ ] 6.2 Add `delivery_pane_wedged` inscription event with target_session,
      pane_target, and last-observed prompt-readiness mismatch reason

## 7. Documentation

- [ ] 7.1 Update the Tmux transport README (if present in `src/tmux/README.md`)
      to describe the three-state model
- [ ] 7.2 Update operator-facing bundle config docs to describe the new keys
      and their disabled-by-default state
- [ ] 7.3 Add a "wedge detection requires correct prompt regex" warning to
      the prompt-readiness docs (cross-reference)
- [ ] 7.4 Add a "operator interaction indefinitely suppresses failure
      classification" note to the Tmux transport docs

## 8. Validation

- [ ] 8.1 `cargo test` green
- [ ] 8.2 `cargo clippy --all-targets` clean
- [ ] 8.3 `cargo fmt --check` clean
- [ ] 8.4 `openspec validate tmux-wedge-detection --strict` passes