//! Test surface for the tmux transport's delivery classifier
//! (running / unresponsive).
//!
//! Tests inject scripted [`agentmux::tmux::PaneQuiescenceProbe`]
//! implementations to drive the classifier deterministically — without
//! real tmux IPC, the probe's state machine is the only seam that can
//! exercise the wait loop's timeout/ready branches.
//!
//! Tmux does not classify `wedged`: it passes `wedge_detection: false`
//! into the shared classifier, so a settled non-prompt frame resolves
//! only through a bound. The scripted probes here therefore cover the
//! prime-timeout and delivery contract:
//!
//! - a probe that never produces output asserts `Timeout`.
//! - a probe that quiesces at a prompt after several ticks asserts
//!   `Delivered`.
//! - a probe that produces output then settles at a prompt asserts
//!   `Delivered` without the prime timeout firing.
//!
//! The readiness bound's own coverage lives in
//! `tests/unit/transports_quiescence.rs`, against the shared classifier
//! both transports drive.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use agentmux::tmux::{
    PaneQuiescenceProbe, PromptReadinessEvaluation, wait_for_quiescent_pane_three_state,
};
use agentmux::transports::{DeliveryDiagnosticContext, DeliveryWaitError, QuiescenceBounds};

const SHORT_QUIET_WINDOW: Duration = Duration::from_millis(5);
const TEST_TARGET_SESSION: &str = "test-session";
const TEST_PANE_TARGET: &str = "%0";

fn diagnostic_context() -> DeliveryDiagnosticContext<'static> {
    DeliveryDiagnosticContext::without_messages("test-namespace", TEST_TARGET_SESSION)
}

/// Test helper: builds the bounds one flush group's wait is subject to.
///
/// Both deadlines derive from a single `Instant::now()`, the way the transport
/// anchors them at group formation, so the diagnostic elapsed values and the
/// two bounds all agree on one origin.
fn tmux_bounds(
    prime_timeout_ms: Option<u64>,
    readiness_timeout_ms: Option<u64>,
) -> QuiescenceBounds {
    QuiescenceBounds::new(
        SHORT_QUIET_WINDOW,
        Instant::now(),
        prime_timeout_ms,
        readiness_timeout_ms,
    )
}

/// Single observation the probe returns. Both query methods
/// (`next_evaluation`, `resolve_active_pane`) return values derived
/// from this struct.
#[derive(Clone, Debug)]
struct ProbeObservation {
    pane_target: String,
    evaluation: PromptReadinessEvaluation,
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
        }
    }

    fn ready() -> Self {
        Self {
            pane_target: TEST_PANE_TARGET.to_string(),
            evaluation: PromptReadinessEvaluation {
                ready: true,
                ..Default::default()
            },
        }
    }

    /// Cursor-class mismatch: regex matched but the cursor column did
    /// not match the per-coder `expected_cursor_column`. Mirrors the
    /// `prompt_readiness_matches` output for
    /// `input_idle_cursor_column`-constrained configs in
    /// `src/tmux/transport.rs::prompt_readiness_matches`. The mismatch
    /// reason must propagate through the wedge state machine unchanged
    /// so operators can distinguish cursor mismatches from regex
    /// mismatches in diagnostics.
    fn cursor_mismatch() -> Self {
        Self {
            pane_target: TEST_PANE_TARGET.to_string(),
            evaluation: PromptReadinessEvaluation {
                ready: false,
                mismatch_reason: Some("cursor column 0 did not match required 4".to_string()),
                inspected_block: Some("> ".to_string()),
                regex_matched: Some(true),
                expected_cursor_column: Some(4),
                observed_cursor_column: Some(0),
            },
        }
    }
}

/// Scripted probe: a queue of observations that the wait loop drains via
/// `wait_for_change`. While the queue has more than one element, every
/// `wait_for_change` call advances the head and returns `Ok(())`. When the
/// queue is at its tail, `wait_for_change` returns `Err(Timeout)`.
///
/// An optional `abort_after_calls` counter makes `next_evaluation` return
/// `Err` after N calls, used to terminate the wait function at a known
/// iteration count without firing wedge or timeout.
struct ScriptedProbe {
    states: VecDeque<ProbeObservation>,
    /// Terminal-output-write marker sequence consumed on every
    /// `last_window_activity_marker()` call. Default-initialized
    /// to a single `0` element so existing tests continue to
    /// not advance activity between observations (the cross-
    /// transport classifier's Busy pre-classification sees no
    /// comparator advance and silently disables itself). Tests
    /// driving the Busy short-circuit use
    /// `with_activity_sequence(...)` to provide a scripted
    /// sequence.
    activity_states: VecDeque<u64>,
    abort_after_calls: Option<usize>,
    call_count: usize,
}

impl ScriptedProbe {
    /// Probe that always returns the same observation.
    fn constant(observation: ProbeObservation) -> Self {
        Self {
            states: VecDeque::from([observation]),
            // Default to a constant-zero activity marker so the
            // cross-transport classifier's Busy pre-classification
            // sees no comparator advance and silently disables
            // itself; this preserves pre-change behavior for
            // existing tests.
            activity_states: VecDeque::from([0u64]),
            abort_after_calls: None,
            call_count: 0,
        }
    }

    /// Probe that advances through a sequence of observations each time
    /// `wait_for_change` is called.
    fn sequence(observations: Vec<ProbeObservation>) -> Self {
        // Default-init activity to a constant-zero sequence
        // (twice the length, so `last_window_activity_marker`'s
        // consume-on-each-observe semantics stays in zero
        // territory). Tests use `with_activity_sequence(...)` to
        // override when exercising the Busy short-circuit.
        let activity_states =
            std::iter::repeat_n(0u64, observations.len().saturating_mul(2)).collect();
        Self {
            states: VecDeque::from(observations),
            activity_states,
            abort_after_calls: None,
            call_count: 0,
        }
    }

    /// Replaces the activity-marker sequence with the supplied
    /// values. Each `last_window_activity_marker()` call consumes
    /// one value; when the queue is exhausted, the probe falls
    /// back to `Some(0)`. Tests exercising the Busy short-
    /// circuit pre-classification supply a sequence where the
    /// value differs between consecutive calls within a single
    /// `quiescence_classify_step` (which has two `observe()`
    /// calls) so the cross-transport classifier's comparator
    /// registers an advance.
    fn with_activity_sequence(mut self, values: Vec<u64>) -> Self {
        self.activity_states = VecDeque::from(values);
        self
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

    fn resolve_active_pane(&mut self) -> Result<String, String> {
        Ok(self.current().pane_target.clone())
    }

    fn last_window_activity_marker(&mut self) -> Result<Option<u64>, String> {
        // Consume (pop_front) so two consecutive
        // `last_window_activity_marker()` calls within a single
        // `quiescence_classify_step` can return different
        // values — the cross-transport classifier's Busy short-
        // circuit compares the two. Empty queue => `Some(0)`
        // (constant activity, no comparator advance, Busy
        // silently disabled).
        Ok(self.activity_states.pop_front().or(Some(0)))
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
        &diagnostic_context(),
        &tmux_bounds(Some(30), None),
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout, got {result:?}",
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
        &diagnostic_context(),
        &tmux_bounds(Some(10_000), None),
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
        &diagnostic_context(),
        &tmux_bounds(Some(10_000), None),
    );
    assert!(
        matches!(result, Ok(ref pane) if pane == TEST_PANE_TARGET),
        "expected Ok(pane), got {result:?}",
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
        &diagnostic_context(),
        &tmux_bounds(None, None),
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
        &diagnostic_context(),
        &tmux_bounds(Some(30), None),
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout after prime window, got {result:?}",
    );
}

/// Same cursor-class probe, but with wedge detection disabled so the
/// state machine takes the prime-timeout branch instead. Verifies the
/// `Timeout` `mismatch_reason` also preserves the cursor-class reason
/// (the same diagnostic path passes through `delivery_prime_timeout`).
#[test]
fn cursor_mismatch_preserves_its_reason_in_timeout_outcome() {
    let mut probe = ScriptedProbe::sequence(vec![
        ProbeObservation::cursor_mismatch(),
        ProbeObservation::cursor_mismatch(),
    ]);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        &diagnostic_context(),
        &tmux_bounds(Some(10), None),
    );
    match result {
        Err(DeliveryWaitError::Timeout {
            mismatch_reason: Some(reason),
            ..
        }) => {
            assert!(
                reason.contains("cursor column"),
                "expected cursor-column reason, got {reason:?}",
            );
        }
        other => panic!("expected Timeout with cursor reason, got {other:?}"),
    }
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
        &diagnostic_context(),
        &tmux_bounds(Some(10), None),
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
    let bounds = QuiescenceBounds {
        quiet_window: SHORT_QUIET_WINDOW,
        started_at: past_now - Duration::from_secs(60),
        prime_deadline: Some(past_now),
        prime_timeout_ms: Some(0),
        readiness_deadline: None,
    };
    let result = wait_for_quiescent_pane_three_state(&mut probe, &diagnostic_context(), &bounds);
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout from pre-elapsed prime deadline, got {result:?}",
    );
}

/// R1 integration test: branch-ordering contract via the Tmux probe
/// path. The probe's every observation is prompt-ready, so the
/// `delivery_ready` branch WOULD resolve as `Ok(pane)` after the
/// first iteration absent Busy. Activity marker advances on every
/// observation (consume-on-each-observe = 2 consumes per quiescence
/// iteration). The Busy short-circuit must fire every iteration,
/// deferring the `Ok(pane)` resolution indefinitely within the test
/// window. Net assertion: the wait function does NOT return
/// `Ok(_)`.
#[test]
fn tmux_busy_short_circuit_defers_delivered_when_activity_advances_while_ready() {
    let observations = vec![ProbeObservation::ready(); 40];
    let activity: Vec<u64> = (1..=80).collect();
    let mut probe = ScriptedProbe::sequence(observations)
        .with_activity_sequence(activity)
        .with_abort_after(60);
    let result = wait_for_quiescent_pane_three_state(
        &mut probe,
        &diagnostic_context(),
        &tmux_bounds(None, None),
    );
    assert!(
        result.is_err(),
        "Busy must have deferred Delivered (branch-ordering contract), got {result:?}",
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Failed { .. })),
        "expected Failed (from abort_after), got {result:?}",
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

/// The Tmux transport's per-envelope paste rendering prepends a marker
/// line to every receipt envelope (`DeliveryEnvelope.is_receipt == true`)
/// so the receiving agent can distinguish a terminal-outcome receipt
/// from a peer message at a glance. The marker is included in the
/// rendered text so the token-budget batching and paste-budget counts
/// stay consistent with the actual pane bytes. Peer envelopes render
/// unchanged. Mirrors the Pty equivalent
/// `pty_transport_start_envelope_group_emits_receipt_marker_for_receipt_only`
/// in `tests/unit/pty_transport.rs`.
#[test]
fn tmux_transport_render_paste_text_emits_receipt_marker_for_receipt_only() {
    use agentmux::envelope::AddressIdentity;
    use agentmux::tmux::render_paste_text;
    use agentmux::transports::{DeliveryEnvelope, DeliveryMessage};

    const RECEIPT_MARKER_LINE: &str = "--- agentmux terminal-outcome receipt ---\n";

    fn make_envelope(is_receipt: bool) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: format!("msg-{}", if is_receipt { "receipt" } else { "peer" }),
            message: DeliveryMessage {
                body: "test body".to_string(),
                created_at: "2026-07-16T00:00:00Z".to_string(),
                namespace: "test-ns".to_string(),
                sender: AddressIdentity {
                    session_name: "alpha@test-ns".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "target@test-ns".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            quiet_window: std::time::Duration::from_millis(50),
            prime_timeout_ms: None,
            readiness_timeout_ms: None,
            is_receipt,
        }
    }

    // Receipt envelope: marker line is emitted immediately before the
    // rendered pane text (which starts with `--<boundary>\n`).
    let receipt_text = render_paste_text(&make_envelope(true));
    assert!(
        receipt_text.starts_with(RECEIPT_MARKER_LINE),
        "receipt envelope must be preceded by the marker line; got: {receipt_text:?}"
    );
    let after_marker = &receipt_text[RECEIPT_MARKER_LINE.len()..];
    assert!(
        after_marker.starts_with("--"),
        "marker must be immediately before the envelope text; got: {after_marker:?}"
    );

    // Peer envelope: marker line is absent.
    let peer_text = render_paste_text(&make_envelope(false));
    assert!(
        !peer_text.contains(RECEIPT_MARKER_LINE),
        "peer envelope must not include the marker line; got: {peer_text:?}"
    );
}
