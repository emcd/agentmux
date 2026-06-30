//! Test surface for the tmux transport's three-state delivery classifier
//! (running / unresponsive / wedged).
//!
//! Tests inject scripted [`agentmux::tmux::PaneQuiescenceProbe`]
//! implementations to drive the classifier deterministically — without
//! real tmux IPC, the probe's state machine is the only seam that can
//! exercise the wait loop's wedge/timeout/ready branches.
//!
//! See the `tmux-wedge-detection` proposal for the behavioral contract;
//! this file is the executable specification. The five-probe surface
//! covers the design.md Decision 6 test plan:
//!
//! - `AlwaysUnresponsiveProbe` — never produces output. Asserts `Timeout`.
//! - `AlwaysWedgeProbe` — immediately quiesces at a non-prompt state.
//!   Asserts `Failed` + `reason_code = "pane_wedged"`.
//! - `PendingChoiceProbe` — quiesces with `operator_interaction_active =
//!   Some(_)`. Asserts neither timeout nor wedge fire while operator
//!   interaction is active.
//! - `SlowPromptProbe` — quiesces at a prompt state after several ticks.
//!   Asserts `Delivered`.
//! - `NormalFlowProbe` — produces output then settles at a prompt.
//!   Asserts `Delivered` without prime or wedge firing.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use agentmux::tmux::{
    PaneQuiescenceProbe, PromptReadinessEvaluation, wait_for_quiescent_pane_three_state,
};
use agentmux::transports::DeliveryWaitError;

const SHORT_QUIET_WINDOW: Duration = Duration::from_millis(5);
const TEST_TARGET_SESSION: &str = "test-session";
const TEST_PANE_TARGET: &str = "%0";

/// Test helper: builds the `(prime_deadline, prime_started_at, prime_timeout_ms)`
/// triple the wait function expects. The captured `Instant::now()` is
/// returned as the `prime_started_at` anchor so the diagnostic
/// `prime_wait_elapsed_ms` is correctly computed.
fn prime_window(timeout_ms: Option<u64>) -> (Option<Instant>, Instant, Option<u64>) {
    let now = Instant::now();
    (
        timeout_ms.map(|ms| now + Duration::from_millis(ms)),
        now,
        timeout_ms,
    )
}

/// Single observation the probe returns. All three query methods
/// (`next_evaluation`, `operator_interaction_active`, `resolve_active_pane`)
/// return values derived from this struct.
#[derive(Clone, Debug)]
struct ProbeObservation {
    pane_target: String,
    evaluation: PromptReadinessEvaluation,
    operator_interaction: Option<String>,
}

impl ProbeObservation {
    fn empty_unready() -> Self {
        Self {
            pane_target: TEST_PANE_TARGET.to_string(),
            evaluation: PromptReadinessEvaluation {
                ready: false,
                mismatch_reason: Some(
                    "inspected pane tail was empty after trimming trailing blank lines".to_string(),
                ),
                ..Default::default()
            },
            operator_interaction: None,
        }
    }

    fn stuck_unready() -> Self {
        Self {
            pane_target: TEST_PANE_TARGET.to_string(),
            evaluation: PromptReadinessEvaluation {
                ready: false,
                mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
                inspected_block: Some("Do you want to proceed? [Y/n]".to_string()),
                regex_matched: Some(false),
                ..Default::default()
            },
            operator_interaction: None,
        }
    }

    fn ready() -> Self {
        Self {
            pane_target: TEST_PANE_TARGET.to_string(),
            evaluation: PromptReadinessEvaluation {
                ready: true,
                ..Default::default()
            },
            operator_interaction: None,
        }
    }

    fn with_op_interaction(mut self, op: &str) -> Self {
        self.operator_interaction = Some(op.to_string());
        self
    }
}

/// Scripted probe: a queue of observations that the wait loop drains via
/// `wait_for_change`. While the queue has more than one element, every
/// `wait_for_change` call advances the head and returns `Ok(())`. When the
/// queue is at its tail, `wait_for_change` returns `Err(Timeout)`.
///
/// An optional `abort_after_calls` counter makes `next_evaluation` return
/// `Err` after N calls, used by `PendingChoiceProbe` to terminate the
/// wait function without firing wedge or timeout.
struct ScriptedProbe {
    states: VecDeque<ProbeObservation>,
    abort_after_calls: Option<usize>,
    call_count: usize,
}

impl ScriptedProbe {
    /// Probe that always returns the same observation.
    fn constant(observation: ProbeObservation) -> Self {
        Self {
            states: VecDeque::from([observation]),
            abort_after_calls: None,
            call_count: 0,
        }
    }

    /// Probe that advances through a sequence of observations each time
    /// `wait_for_change` is called.
    fn sequence(observations: Vec<ProbeObservation>) -> Self {
        Self {
            states: VecDeque::from(observations),
            abort_after_calls: None,
            call_count: 0,
        }
    }

    fn with_abort_after(mut self, calls: usize) -> Self {
        self.abort_after_calls = Some(calls);
        self
    }

    fn current(&self) -> &ProbeObservation {
        self.states.front().expect("at least one observation")
    }
}

impl PaneQuiescenceProbe for ScriptedProbe {
    fn next_evaluation(&mut self) -> Result<PromptReadinessEvaluation, String> {
        self.call_count += 1;
        if let Some(max) = self.abort_after_calls
            && self.call_count > max
        {
            return Err("scripted probe: abort after configured call count".to_string());
        }
        Ok(self.current().evaluation.clone())
    }

    fn operator_interaction_active(&mut self) -> Result<Option<String>, String> {
        Ok(self.current().operator_interaction.clone())
    }

    fn resolve_active_pane(&mut self) -> Result<String, String> {
        Ok(self.current().pane_target.clone())
    }

    fn wait_for_change(&mut self, _deadline: Instant) -> Result<(), DeliveryWaitError> {
        if self.states.len() > 1 {
            self.states.pop_front();
            Ok(())
        } else {
            // At the tail — no further state to advance to. Surface Timeout
            // so the wait function's existing prime-deadline check decides
            // whether to fire Timeout or loop back.
            Err(DeliveryWaitError::Timeout {
                timeout: Duration::from_millis(0),
                readiness_mismatch: false,
                mismatch_reason: None,
            })
        }
    }
}

#[test]
fn always_unresponsive_probe_resolves_timeout() {
    // Pane is stuck at an empty non-prompt state with no operator
    // interaction. Wedge detection disabled so the prime window is the
    // sole bound — fires `Timeout` once the prime window elapses with
    // no observed change.
    let mut probe = ScriptedProbe::constant(ProbeObservation::empty_unready());
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(30)).0,
        prime_window(Some(30)).1,
        prime_window(Some(30)).2,
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout, got {result:?}",
    );
}

#[test]
fn always_wedge_probe_resolves_wedged() {
    // Pane immediately quiesces at a non-prompt state (a stuck-on-
    // permission-dialog mismatch). Wedge detection enabled and the
    // mismatch signature stays the same across ticks, so the wedge
    // counter accumulates to the threshold and fires Wedged before the
    // prime window expires.
    let mut probe = ScriptedProbe::constant(ProbeObservation::stuck_unready());
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged, got {result:?}",
    );
}

#[test]
fn pending_choice_probe_neither_timeout_nor_wedge() {
    // Pane has `operator_interaction_active` set (a pending tool-approval
    // choice). Both the unresponsive and the wedged classifications are
    // indefinitely suppressed. We use `abort_after_calls` to terminate the
    // wait function with a distinct `Failed` variant, proving neither
    // Timeout nor Wedged fired during the run.
    let mut probe = ScriptedProbe::constant(
        ProbeObservation::stuck_unready().with_op_interaction("pane_in_mode"),
    )
    .with_abort_after(20);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(30)).0,
        prime_window(Some(30)).1,
        prime_window(Some(30)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Failed { .. })),
        "expected Failed (from abort), got {result:?}",
    );
    assert!(
        !matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "must not fire Timeout while operator interaction is active",
    );
    assert!(
        !matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "must not fire Wedged while operator interaction is active",
    );
}

#[test]
fn slow_prompt_probe_resolves_delivered() {
    // Pane transitions through distinct non-prompt states (empty → stuck)
    // before finally reaching a ready state. Each mismatch signature
    // change resets the wedge counter, so wedge does not fire even with
    // wedge detection enabled.
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::empty_unready(),
        ProbeObservation::empty_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::ready(),
    ]);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Ok(ref pane) if pane == TEST_PANE_TARGET),
        "expected Ok(pane), got {result:?}",
    );
}

#[test]
fn normal_flow_probe_resolves_delivered() {
    // Pane shows changing non-prompt output before settling at a prompt
    // state. The wedge counter resets on each signature change; the
    // prime window is generous enough that Timeout does not fire before
    // the ready state arrives.
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::empty_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::ready(),
    ]);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        false,
    );
    assert!(
        matches!(result, Ok(ref pane) if pane == TEST_PANE_TARGET),
        "expected Ok(pane), got {result:?}",
    );
}

#[test]
fn wedge_default_on_fires_after_consecutive_identical_mismatches() {
    // Verifies the default-on wedge behavior: with wedge_detection = true
    // and a sustained non-prompt state, Wedged fires after the wedge
    // counter reaches the consecutive-tick threshold.
    let mut probe = ScriptedProbe::constant(ProbeObservation::stuck_unready());
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged, got {result:?}",
    );
}

#[test]
fn wedge_disabled_preserves_unbounded_wait() {
    // With wedge disabled and no prime timeout, the wait function does
    // NOT fire Timeout or Wedged even when the pane is at a sustained
    // non-prompt state. We use `abort_after_calls` to terminate the
    // wait function with `Failed` and prove neither Timeout nor Wedged
    // fired during the run.
    let mut probe = ScriptedProbe::constant(ProbeObservation::stuck_unready()).with_abort_after(20);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(None).0,
        prime_window(None).1,
        prime_window(None).2,
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Failed { .. })),
        "expected Failed (abort), got {result:?}",
    );
    assert!(
        !matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "must not fire Timeout when no prime timeout is configured",
    );
    assert!(
        !matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "must not fire Wedged when wedge detection is disabled",
    );
}

#[test]
fn prime_timeout_default_off_does_not_fire() {
    // No prime timeout configured. Even with the pane at a sustained
    // non-prompt state and wedge disabled, the wait function does not
    // fire Timeout.
    let mut probe = ScriptedProbe::constant(ProbeObservation::empty_unready()).with_abort_after(20);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(None).0,
        prime_window(None).1,
        prime_window(None).2,
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Failed { .. })),
        "expected Failed (abort), got {result:?}",
    );
    assert!(
        !matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "must not fire Timeout when prime_timeout_ms is absent",
    );
}

#[test]
fn prime_timeout_opt_in_fires_after_window() {
    // Prime timeout configured to a short window. With wedge disabled
    // and a sustained non-prompt state, Timeout fires once the window
    // elapses with no observed change.
    let mut probe = ScriptedProbe::constant(ProbeObservation::empty_unready());
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(30)).0,
        prime_window(Some(30)).1,
        prime_window(Some(30)).2,
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout after prime window, got {result:?}",
    );
}

#[test]
fn coalesce_during_wedge_counter_does_not_fire_on_changing_signatures() {
    // Verifies that the wedge counter resets when the mismatch signature
    // changes between ticks — agents transitioning through distinct
    // non-prompt states (e.g. boot output, tool-call prep, idle screens)
    // must not accumulate wedge ticks.
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::empty_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::ready(),
    ]);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Ok(ref pane) if pane == TEST_PANE_TARGET),
        "expected Delivered after signature changes, got {result:?}",
    );
}

#[test]
fn coalesce_during_wedge_counter_fires_after_consecutive_identical_signatures() {
    // Counterpart to the prior test: when the mismatch signature stays
    // the same across ticks, the wedge counter accumulates and Wedged
    // fires at the threshold even if the probe would later transition to
    // a ready state.
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::stuck_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::stuck_unready(),
        ProbeObservation::ready(),
    ]);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged after consecutive identical signatures, got {result:?}",
    );
}

/// Verifies the spec requirement that the prime timer is anchored to
/// the delivery-task perspective and does NOT reset across coalesce
/// iterations. The test passes a `prime_deadline` that has ALREADY
/// elapsed (computed against a captured `Instant::now()` from well in
/// the past) and asserts the wait function fires Timeout on the very
/// first iteration. If the deadline were reset on each call (i.e.
/// re-captured from `Instant::now()` inside the wait function), the
/// wait would loop indefinitely instead of firing Timeout.
/// Verifies BE's spec interpretation: a short prime timeout MUST NOT
/// pre-empt wedge for a quiesced pane showing wedge-class content
/// (regex/cursor mismatch). The probe stays at `stuck_unready` (a
/// wedge-class mismatch) for the entire run; wedge detection is
/// enabled; the prime timeout is shorter than the wedge counter
/// threshold. Expected outcome: `Wedged` (not `Timeout`).
#[test]
fn short_prime_timeout_does_not_preempt_wedge_for_wedge_class_mismatch() {
    let mut probe = ScriptedProbe::constant(ProbeObservation::stuck_unready());
    // Short prime window (10ms) — would fire before the wedge counter
    // reaches WEDGE_CONSECUTIVE_TICKS (3 ticks × 5ms = 15ms).
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10)).0,
        prime_window(Some(10)).1,
        prime_window(Some(10)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged (wedge-class mismatch fires Wedged regardless of prime window), got {result:?}",
    );
}

/// Counterpart to the prior test: a short prime timeout with a
/// dead-pane probe (empty mismatch, no observable content) fires
/// `Timeout`, not `Wedged`. Empty mismatches are Unresponsive
/// territory; wedge detection does not apply.
#[test]
fn short_prime_timeout_fires_timeout_for_dead_pane_mismatch() {
    let mut probe = ScriptedProbe::constant(ProbeObservation::empty_unready());
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10)).0,
        prime_window(Some(10)).1,
        prime_window(Some(10)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (dead-pane mismatch fires Timeout), got {result:?}",
    );
}

#[test]
fn coalesce_during_prime_does_not_extend_window() {
    // Build a probe that takes a long time per iteration so the wait
    // would naturally span multiple coalesce iterations if the
    // prime_deadline were re-captured.
    //
    // Uses an empty-pane (non-wedge-class) probe so the prime timeout
    // fires `Timeout`; a wedge-class probe would fire `Wedged` per the
    // precedence rules (see `short_prime_timeout_*` tests).
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::empty_unready(),
        ProbeObservation::empty_unready(),
        ProbeObservation::empty_unready(),
        ProbeObservation::empty_unready(),
    ]);
    // Capture a "now" that's already past the deadline we construct.
    // The wait function MUST use the passed deadline (not re-capture
    // `Instant::now()`) for this assertion to hold.
    let past_now = Instant::now() - Duration::from_secs(60);
    let already_elapsed_deadline = past_now;
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        Some(already_elapsed_deadline),
        past_now - Duration::from_secs(60),
        Some(0),
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout from pre-elapsed prime deadline, got {result:?}",
    );
}

/// Group atomicity test (5.8): when the wedge classifier fires for a
/// coalesced flush group, the entire group resolves with the same
/// `pane_wedged` outcome. The wait function itself only returns one
/// error per call; `flush_and_resolve` maps that error to every
/// sender in the group. This test exercises the mapping logic
/// through `wait_error_to_outcome` directly.
#[test]
fn wedge_outcome_maps_to_pane_wedged_reason_code() {
    use agentmux::tmux::wait_error_to_outcome_for_test;
    let outcome = wait_error_to_outcome_for_test(
        TEST_TARGET_SESSION,
        &DeliveryWaitError::Wedged {
            reason: "prompt regex did not match inspected pane tail".to_string(),
        },
        "msg-1",
    );
    use agentmux::transports::SendOutcome;
    assert_eq!(outcome.outcome, SendOutcome::Failed);
    assert_eq!(outcome.reason_code.as_deref(), Some("pane_wedged"));
    assert!(
        outcome
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("prompt regex did not match inspected pane tail"),
        "reason should carry the wedge diagnostic, got {:?}",
        outcome.reason,
    );
}

/// Prime timeout outcome mapping: Timeout fires with reason_code
/// `delivery_prime_timeout` and `SendOutcome::Timeout`.
#[test]
fn timeout_outcome_maps_to_prime_timeout_reason_code() {
    use agentmux::tmux::wait_error_to_outcome_for_test;
    use agentmux::transports::SendOutcome;
    let outcome = wait_error_to_outcome_for_test(
        TEST_TARGET_SESSION,
        &DeliveryWaitError::Timeout {
            timeout: Duration::from_millis(250),
            readiness_mismatch: true,
            mismatch_reason: Some("inspected pane tail was empty".to_string()),
        },
        "msg-1",
    );
    assert_eq!(outcome.outcome, SendOutcome::Timeout);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("delivery_prime_timeout"),
    );
}
