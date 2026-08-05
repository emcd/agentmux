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

use agentmux::envelope::PromptBatchSettings;
use agentmux::tmux::{
    PaneQuiescenceProbe, PromptReadinessEvaluation, wait_for_quiescent_pane_three_state,
};
use agentmux::transports::{
    DeliveryDiagnosticContext, DeliveryWaitError, QuiescenceBounds, Transport,
};

const SHORT_QUIET_WINDOW: Duration = Duration::from_millis(5);
const TEST_TARGET_SESSION: &str = "test-session";
const TEST_PANE_TARGET: &str = "%0";

#[test]
fn tmux_handover_is_not_accepted_before_startup() {
    let transport = agentmux::tmux::TmuxTransport::new(PromptBatchSettings::default());

    assert!(!transport.is_ready_for_handover());
}

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
    // before finally reaching a ready state. Tmux passes
    // `wedge_detection: false`, so no wedge verdict is reachable here at all;
    // the transitions matter because each must leave the group pending rather
    // than resolving it early.
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
    // fires `Timeout`. A wedge-class probe would not resolve here at all on
    // Tmux: the wedge verdict is unreachable, and an elapsed prime timeout is
    // excluded from settled wedge-class frames on a group that carries a
    // readiness bound, which every Tmux group does.
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

/// The tmux fence's forced step must reach a thread parked inside a tmux client
/// call.
///
/// Dropping the write channel — the only thing termination used to do — returns
/// a delivery thread waiting for its *next* item. It reaches nothing at all for
/// one already blocked waiting on a `tmux` invocation, which is precisely the
/// case that made the cooperative step fail and the escalation necessary. So the
/// discriminating sequence is: cooperative request, observe still-executing,
/// forced step, observe ceased. Without signalling the invocation the last
/// observation never arrives.
///
/// The fake tmux blocks on opening a fifo with no writer rather than sleeping: a
/// `sleep` would be a separate process inheriting the invocation's stdout, so
/// killing the invocation would leave the pipe open and the waiting thread would
/// stay blocked reading it — masking exactly what this asserts.
#[test]
fn a_parked_tmux_invocation_ceases_only_under_forced_termination() {
    use agentmux::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
    use agentmux::transports::{GenerationFence, StartupContext};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let temporary = tempfile::TempDir::new().expect("temporary");
    let fifo = temporary.path().join("block.fifo");
    let entered = temporary.path().join("entered");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    std::fs::write(
        &fake_tmux,
        format!(
            "#!/bin/sh\n\
             : > '{entered}'\n\
             [ -p '{fifo}' ] || mkfifo '{fifo}'\n\
             read line < '{fifo}'\n",
            entered = entered.display(),
            fifo = fifo.display(),
        ),
    )
    .expect("write fake tmux");
    std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755))
        .expect("make fake tmux executable");
    // SAFETY: nextest runs each test in its own process, so no other thread here
    // races this read of the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let member = BundleMember {
        id: TEST_TARGET_SESSION.to_string(),
        name: None,
        working_directory: None,
        target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
            start_command: "/bin/sh".to_string(),
            prompt_readiness: None,
            prime_timeout_ms: None,
            readiness_timeout_ms: 1_000,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::tmux::TmuxTransport::new(PromptBatchSettings::default());
    transport
        .startup(StartupContext {
            namespace: "party".to_string(),
            runtime_directory: temporary.path().to_path_buf(),
            target_member: member,
            choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
                decided_by: "test".to_string(),
                reason_code: "test_cancel".to_string(),
                reason: None,
            }),
        })
        .expect("tmux startup");

    let _outcome = transport.raww("hello".to_string(), true);
    await_path(
        &entered,
        "the delivery thread should have entered a tmux invocation",
    );

    // Step 1: the cooperative request cannot reach a thread blocked in a syscall,
    // so the generation is still executing after it.
    transport.fence_generation();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !transport.generation_ceased(),
        "a thread parked in a tmux invocation cannot observe the cooperative flag"
    );

    // Step 3: signalling the invocation is what lets the observation succeed.
    transport.terminate_generation();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.generation_ceased() {
        assert!(
            Instant::now() < deadline,
            "the generation did not cease within 5s of forced termination"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The fence's forced step must reach a thread parked *writing into* a tmux
/// client, not only one parked waiting on its exit.
///
/// A paste loads its text through `load-buffer -` over the client's stdin. A
/// client that stops reading lets that pipe fill, and the delivery thread then
/// blocks inside `write_all` with nothing left to interrupt it — the same
/// unreachable state the previous test covers, arrived at by a different route.
/// The invocation used to be published only after the write returned, so for the
/// whole duration of that block the slot the forced step reads was empty and the
/// step had nothing to signal. Publishing before the first byte is what closes
/// it; revert that and the last observation below never arrives.
///
/// The payload has to exceed the pipe capacity, or the write completes into the
/// buffer and the thread parks somewhere already covered. The fake client blocks
/// on a fifo without reading its stdin, so the fill is real rather than timed.
#[test]
fn a_tmux_paste_write_is_reachable_before_its_first_byte() {
    use agentmux::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
    use agentmux::transports::{GenerationFence, StartupContext};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let payload_bytes = blocking_payload_bytes();

    let temporary = tempfile::TempDir::new().expect("temporary");
    let fifo = temporary.path().join("block.fifo");
    let loading = temporary.path().join("loading");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    // Answers pane resolution so the delivery thread gets as far as the paste,
    // then blocks on `load-buffer` *without* reading stdin.
    std::fs::write(
        &fake_tmux,
        format!(
            "#!/bin/sh\n\
             for arg in \"$@\"; do\n\
               if [ \"$arg\" = 'load-buffer' ]; then\n\
                 : > '{loading}'\n\
                 [ -p '{fifo}' ] || mkfifo '{fifo}'\n\
                 read line < '{fifo}'\n\
                 exit 0\n\
               fi\n\
             done\n\
             echo '%0'\n",
            loading = loading.display(),
            fifo = fifo.display(),
        ),
    )
    .expect("write fake tmux");
    std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755))
        .expect("make fake tmux executable");
    // SAFETY: nextest runs each test in its own process, so no other thread here
    // races this read of the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let member = BundleMember {
        id: TEST_TARGET_SESSION.to_string(),
        name: None,
        working_directory: None,
        target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
            start_command: "/bin/sh".to_string(),
            prompt_readiness: None,
            prime_timeout_ms: None,
            readiness_timeout_ms: 1_000,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::tmux::TmuxTransport::new(PromptBatchSettings::default());
    transport
        .startup(StartupContext {
            namespace: "party".to_string(),
            runtime_directory: temporary.path().to_path_buf(),
            target_member: member,
            choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
                decided_by: "test".to_string(),
                reason_code: "test_cancel".to_string(),
                reason: None,
            }),
        })
        .expect("tmux startup");

    let _outcome = transport.raww("x".repeat(payload_bytes), false);
    await_path(
        &loading,
        "the delivery thread should have reached the paste's load-buffer",
    );

    // The write is parked with the pipe full, and the cooperative flag reaches a
    // thread blocked in a syscall no better here than anywhere else.
    transport.fence_generation();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !transport.generation_ceased(),
        "a thread parked writing into a tmux client cannot observe the \
         cooperative flag"
    );

    transport.terminate_generation();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.generation_ceased() {
        assert!(
            Instant::now() < deadline,
            "the generation did not cease within 5s of forced termination"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An invocation must stay reachable until its exit is actually observed.
///
/// Draining an invocation's pipes and waiting for it to exit are different
/// events, and a tmux client can close its stdio while still running. The reap
/// used to take the child out of the shared slot and only then block in `wait`,
/// so for the whole of that wait the fence's forced step looked into an empty
/// slot and found nothing to signal — while the executor sat on a live process.
///
/// The fake client closes stdout and stderr, then blocks. Draining returns at
/// once on both pipes, which puts the executor into exactly that wait with a
/// live child; the forced step then has to reach it.
#[test]
fn a_tmux_invocation_stays_reachable_until_its_exit_is_observed() {
    use agentmux::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
    use agentmux::transports::{GenerationFence, StartupContext};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let temporary = tempfile::TempDir::new().expect("temporary");
    let fifo = temporary.path().join("block.fifo");
    let entered = temporary.path().join("entered");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    std::fs::write(
        &fake_tmux,
        format!(
            "#!/bin/sh\n\
             : > '{entered}'\n\
             [ -p '{fifo}' ] || mkfifo '{fifo}'\n\
             exec 1>&- 2>&-\n\
             read line < '{fifo}'\n",
            entered = entered.display(),
            fifo = fifo.display(),
        ),
    )
    .expect("write fake tmux");
    std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755))
        .expect("make fake tmux executable");
    // SAFETY: nextest runs each test in its own process, so no other thread here
    // races this read of the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let member = BundleMember {
        id: TEST_TARGET_SESSION.to_string(),
        name: None,
        working_directory: None,
        target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
            start_command: "/bin/sh".to_string(),
            prompt_readiness: None,
            prime_timeout_ms: None,
            readiness_timeout_ms: 1_000,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::tmux::TmuxTransport::new(PromptBatchSettings::default());
    transport
        .startup(StartupContext {
            namespace: "party".to_string(),
            runtime_directory: temporary.path().to_path_buf(),
            target_member: member,
            choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
                decided_by: "test".to_string(),
                reason_code: "test_cancel".to_string(),
                reason: None,
            }),
        })
        .expect("tmux startup");

    let _outcome = transport.raww("hello".to_string(), true);
    await_path(
        &entered,
        "the delivery thread should have entered a tmux invocation",
    );
    // Past the point where both pipes have hit EOF and the executor is waiting
    // on a client that has not exited.
    std::thread::sleep(Duration::from_millis(150));

    transport.fence_generation();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !transport.generation_ceased(),
        "a thread waiting on a live client cannot observe the cooperative flag"
    );

    transport.terminate_generation();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.generation_ceased() {
        assert!(
            Instant::now() < deadline,
            "the generation did not cease within 5s of forced termination"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A payload that cannot fit in a pipe, so a write into one with no reader
/// blocks rather than completing into the buffer.
///
/// Derived from the platform's ceiling rather than assumed: a pipe holds 64 KiB
/// by default on Linux, but a process may enlarge it up to
/// `/proc/sys/fs/pipe-max-size`, and a constant chosen against the default is a
/// constant that stops discriminating the moment that changes. Where the ceiling
/// cannot be read, fall back to a value comfortably past every capacity this
/// project has encountered.
fn blocking_payload_bytes() -> usize {
    const FALLBACK_BYTES: usize = 8 << 20;
    const PAGE_HEADROOM: usize = 4096;

    std::fs::read_to_string("/proc/sys/fs/pipe-max-size")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map_or(FALLBACK_BYTES, |ceiling| ceiling + PAGE_HEADROOM)
}

/// Polls for `path` to appear, panicking with `message` if it does not.
fn await_path(path: &std::path::Path, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
