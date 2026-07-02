## 1. Spec and design

- [x] 1.1 ADD the ACP Prime Timeout requirement to
      `session-relay/spec.md` (the new requirement lives in the
      `acp-prime-timeout-and-wedge-detection/specs/session-relay/spec.md`
      delta and merges into `specs/session-relay/spec.md` on archive)
- [x] 1.2 ADD the ACP Prime Timeout Envelope Field Consumption
      requirement to `transport-abstraction/spec.md` (the new
      requirement lives in the
      `acp-prime-timeout-and-wedge-detection/specs/transport-abstraction/spec.md`
      delta and merges into `specs/transport-abstraction/spec.md`
      on archive)
- [x] 1.3 MODIFY the Send Timeout Override Flags by Transport
      requirement in `cli-surface/spec.md` to retire the
      `--acp-turn-timeout-ms` flag entirely; v1 has no
      transport-scoped timeout override flag
- [x] 1.4 MODIFY the Send Target Selection requirement in
      `mcp-tool-surface/spec.md` to retire the `acp_turn_timeout_ms`
      payload field entirely; v1 has no transport-scoped timeout
      override field
- [x] 1.5 MODIFY the Non-Expiring Choice Pending Lifecycle
      requirement in `session-relay/spec.md` to update the
      parenthetical cross-reference from
      `acp_turn_timeout_ms`/`[coders.acp] turn-timeout-ms` to the
      renamed field name `[coders.acp] prime-timeout-ms` and to
      drop the retired per-call surface from the reference

## 2. Configuration

- [x] 2.1 Rename `AcpTargetConfiguration.turn_timeout_ms` →
      `AcpTargetConfiguration.prime_timeout_ms` in
      `src/configuration/types.rs` (type declaration).
- [x] 2.2 Rename the field in `src/configuration/raw.rs` (the
      `RawAcpTarget` raw loader and `AcpTarget` intermediate
      loader).
- [x] 2.3 Rename the field in `src/configuration/targets.rs`
      (`build_session_target` copy, `validate_acp_target`
      bounds check + error message, and `validate_acp_target`
      copy). Update the validator error message from
      `"ACP turn-timeout-ms must be greater than zero"` to
      `"ACP prime-timeout-ms must be greater than zero"`.
- [x] 2.4 Confirm the rename compiles and that no other
      production code reads `turn_timeout_ms` (a `rg
      turn_timeout_ms src/` search should return zero matches
      after the rename; the validator's error message text is
      the only remaining mention, and it now says
      `prime-timeout-ms`).
- [x] 2.5 Drop the per-call ACP timeout surfaces from
      `src/relay/handlers/send.rs`: remove the read of
      `acp_turn_timeout_ms` from the request payload (the
      field does not exist in v1).
- [x] 2.6 Drop the `--acp-turn-timeout-ms` flag from the CLI
      parser definition in `src/cli/`. Verify that
      `agentmux send --help` no longer lists the flag.
- [x] 2.7 Update operator-facing docs that reference the legacy
      `--acp-turn-timeout-ms` flag and `acp_turn_timeout_ms`
      payload field. Both are retired; the replacement is the
      per-coder `[coders.<id>.acp].prime-timeout-ms` config key.
- [x] 2.8 Update `src/relay/delivery/quiescence.rs::QuiescenceOptions`
      to carry the new `prime_timeout_ms: Option<u64>`
      constructor parameter (this is also done by the
      `tmux-wedge-detection` proposal for the Tmux path; verify
      it lands the field on the ACP path before this proposal
      merges, or coordinate the merge order).

## 3. Transport contract

- [x] 3.1 Confirm `DeliveryEnvelope.prime_timeout_ms: Option<u64>`
      is declared on the envelope
      (`src/transports/contract.rs` — added by the
      `tmux-wedge-detection` proposal; verify the field exists
      before this proposal merges). No contract change required
      in this proposal.
- [x] 3.2 Confirm the field replaces the legacy
      `quiescence_timeout` field (also done by the
      `tmux-wedge-detection` proposal). The ACP path does not
      need to read `quiescence_timeout`.

## 4. ACP delivery refactor

- [x] 4.1 In `src/acp/transport.rs::acp_delivery_task` /
      `submit_envelope_turn`, read `envelope.prime_timeout_ms`
      at turn start (only if the envelope is an `Envelope` item
      with `prime_timeout_ms.is_some()`; raw writes and
      unbounded envelopes preserve today's behavior).
- [x] 4.2 Track the prime timer anchor: start time when first
      `wait_for_prompt_complete` is called for the flush group.
      Do NOT reset on coalesce-during-wait.
- [x] 4.3 On each `wait_for_prompt_complete` poll, check whether
      the prime window has elapsed AND no
      `pending_choice_outcome` is in flight AND no
      `PromptCompletion` has been observed. On fire, resolve the
      flush group with `SendOutcome::Timeout` +
      `reason_code = "acp_turn_timeout"`, set readiness to
      `Unavailable`, emit a `delivery_prime_timeout`
      inscription, and signal respawn-needed via the existing
      `signal_respawn_if_needed` path.
- [x] 4.4 Confirm `acp_delivery_task` does NOT inject further
      messages into the wedge after the prime timer fires. The
      failure is terminal; the relay worker does not re-submit
      until the worker respawns.
- [x] 4.5 Confirm the per-turn readiness transition on
      prime-timer fire is `Busy -> Unavailable` (matching
      `Decision 7` in `design.md`).
- [x] 4.6 Confirm the prime timer starts only after
      `PromptDispatchOutcome::Submitted`; pre-submit failures
      (`TransportUnavailable`, `SerializationFailed`) preserve
      today's behavior and are NOT affected by the prime timer.

## 5. Test coverage

- [x] 5.1 Add `acp_prime_timeout_fires_after_configured_window`
      to `tests/integration/acp/lifecycle.rs`. Set
      `prime-timeout-ms` to a short value; submit a prompt to a
      child that never responds; assert `Timeout` outcome +
      `reason_code = "acp_turn_timeout"` +
      `delivery_prime_timeout` inscription emitted.
- [x] 5.2 Add `acp_prime_timer_does_not_reset_on_coalesce` to
      `tests/integration/acp/lifecycle.rs`. Set
      `prime-timeout-ms` to a finite value; submit two
      envelopes into the same flush group; assert the second
      envelope inherits the head envelope's prime anchor (the
      prime timer does not extend the deadline).
- [x] 5.3 Add `acp_prime_timer_does_not_fire_during_pending_choice`
      to `tests/integration/acp/lifecycle.rs`. Set
      `prime-timeout-ms` to a finite value; raise a permission
      request mid-turn; assert the prime timer does not fire
      while the choice is pending.
- [x] 5.4 Add `acp_prime_timeout_default_unbounded` to
      `tests/integration/acp/lifecycle.rs`. Omit
      `prime-timeout-ms`; submit a prompt that takes a long time
      to complete; assert `Delivered` outcome (prime timer does
      not fire on the default-unbounded path).
- [x] 5.5 Add a fixture-based test for the operator knob rename:
      a bundle config that sets `[coders.<id>.acp]
      turn-timeout-ms` (the legacy name) fails to load with the
      raw loader's `deny_unknown_fields` error. Conversely, a
      bundle config that sets the new key name loads
      successfully.
- [x] 5.8 Add unit tests for the prime-timer anchor calculation
      in `tests/unit/acp_transport.rs` (or the equivalent
      unit-test surface for `acp_delivery_task`). Cover the
      anchor-start, anchor-not-reset-on-coalesce,
      anchor-not-fire-during-pending-choice cases.
- [x] 5.9 Confirm existing `#[ignore]`d ACP tests
      (`acp_load_failure_does_not_fallback_to_session_new`,
      `acp_new_failure_returns_runtime_stage_code`,
      `wait_times_out_while_pending_then_resolves_on_completion`)
      remain `#[ignore]`d until the issues/acp/10 fix
      (commit `06b3bf9`) is confirmed stable across multiple
      concurrent pre-commit runs. Re-enable in a separate
      follow-up commit (this proposal does NOT re-enable them).

## 6. Diagnostics

- [x] 6.1 Emit `delivery_prime_timeout` inscription with
      `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`
      on prime-timer fire. Mirror the Tmux-side inscription
      shape from the `tmux-wedge-detection` proposal Section 6.1.
- [x] 6.2 Confirm `delivery_prime_timeout` is emitted on the ACP
      path with the same field shape as the Tmux path (no
      `acp_`-prefixed fields).

## 7. Documentation

- [x] 7.1 Update `src/acp/README.md` (if present) to describe
      the renamed `[coders.<id>.acp].prime-timeout-ms` field,
      including:
      - the field is the renamed successor to the legacy
        `turn-timeout-ms` knob; the legacy name is no longer
        accepted (`deny_unknown_fields` error at bundle load)
      - the field defaults to absent (unbounded); set to a
        finite millisecond value to opt in
      - the legacy `turn_timeout_ms` typed field was previously
        dead baggage (validated, never consumed); this proposal
        makes the renamed field load-bearing
- [x] 7.2 Update operator-facing bundle config docs to describe
      the ACP prime timeout semantics, cross-referencing the
      Tmux prime timeout and wedge detection (which use the
      `[coders.<id>.tmux].prime-timeout-ms` / `wedge-detection`
      keys introduced by the `tmux-wedge-detection` proposal).
- [x] 7.3 Add a "prime timeout suppresses during pending choice"
      note to the ACP transport docs (cross-reference the
      non-expiring choice pending lifecycle contract).
- [x] 7.4 Document the per-call override retirement in the
      `agentmux send` help text and the MCP `send` payload docs:
      `send` carries no per-call timeout override field in v1;
      the per-coder config is the only timeout surface.

## 8. Validation

- [x] 8.1 `cargo test` green
- [x] 8.2 `cargo clippy --all-targets` clean
- [x] 8.3 `cargo fmt --check` clean
- [x] 8.4 `openspec validate acp-prime-timeout-and-wedge-detection
      --strict` passes
- [x] 8.5 Pre-commit hooks pass
- [x] 8.6 Confirm the three `#[ignore]`d ACP tests remain
      `#[ignore]`d and the issues/acp/10 fix from commit
      `06b3bf9` is unchanged