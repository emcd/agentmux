//! Cross-transport wedge/prime quiescence classifier tests.
//!
//! These tests exercise [`agentmux::transports::quiescence_classify_step`]
//! directly through a mock [`WedgeProbe`], validating the Busy
//! short-circuit branch and the branch-ordering contract without
//! relying on a specific transport's plumbing.
//!
//! The Busy pre-classification lives in the cross-transport classifier
//! (`src/transports/quiescence.rs::quiescence_classify_step`); the
//! transport-specific `observe()` implementations (Tmux's
//! `TmuxAsWedgeProbe`, Pty's `PtyQuiescenceProbe` /
//! `WorkerTerminalProbe`) populate the [`WedgeObservation`] fields
//! their transport can observe. Testing the classifier through a mock
//! keeps these tests independent of which transport primitives
//! surface the field, so the cross-transport contract is verified
//! independently of the per-transport integration tests in
//! `tests/unit/tmux_transport.rs` and `tests/unit/pty_transport.rs`.
use std::time::{Duration, Instant};

use agentmux::envelope::AddressIdentity;
use agentmux::transports::{
    DIAGNOSTIC_MESSAGE_IDS_MAXIMUM, DeliveryDiagnosticContext, DeliveryEnvelope, DeliveryMessage,
    DeliveryWaitError, QuiescenceAction, QuiescenceBounds, QuiescenceState, ReadinessMismatch,
    ReadinessTimeoutReason, WEDGE_CONSECUTIVE_TICKS, WedgeObservation, WedgeProbe,
    classify_readiness_timeout_reason, quiescence_classify_step,
};

const TEST_TARGET_SESSION: &str = "test-session";
const TEST_QUIET_WINDOW: Duration = Duration::from_millis(1);
const TEST_PANE: &str = "%0";

fn diagnostic_context() -> DeliveryDiagnosticContext<'static> {
    DeliveryDiagnosticContext::without_messages("test-namespace", TEST_TARGET_SESSION)
}

/// Test helper: bounds for one flush group, anchored at `started_at`.
fn quiescence_bounds(
    started_at: Instant,
    prime_timeout_ms: Option<u64>,
    readiness_timeout_ms: Option<u64>,
) -> QuiescenceBounds {
    QuiescenceBounds::new(
        TEST_QUIET_WINDOW,
        started_at,
        prime_timeout_ms,
        readiness_timeout_ms,
    )
}

#[test]
fn diagnostic_context_caps_ids_and_preserves_total() {
    let ids: Vec<String> = (0..DIAGNOSTIC_MESSAGE_IDS_MAXIMUM + 3)
        .map(|index| format!("message-{index}"))
        .collect();
    let context = DeliveryDiagnosticContext::new(
        "test-namespace",
        TEST_TARGET_SESSION,
        ids.iter().map(String::as_str),
    );

    assert_eq!(context.namespace, "test-namespace");
    assert_eq!(context.target_session, TEST_TARGET_SESSION);
    assert_eq!(context.message_ids().len(), DIAGNOSTIC_MESSAGE_IDS_MAXIMUM);
    assert_eq!(context.message_ids_total(), ids.len());
    assert_eq!(
        context.message_ids().first().map(String::as_str),
        Some("message-0")
    );
    assert_eq!(
        context.message_ids().last().map(String::as_str),
        Some(format!("message-{}", DIAGNOSTIC_MESSAGE_IDS_MAXIMUM - 1).as_str()),
    );
}

fn make_observation(activity_generation: u64, is_prompt_ready: bool) -> WedgeObservation {
    WedgeObservation {
        inspected_tail: String::new(),
        is_prompt_ready,
        pane_target: Some(TEST_PANE.to_string()),
        mismatch: None,
        activity_generation,
    }
}

/// Mock cross-transport [`WedgeProbe`] that returns `first` on the
/// first `observe()` call and `later` on every subsequent call
/// (including the second `observe()` within a single
/// `quiescence_classify_step`). `wait_for_change` returns `Ok(())` so
/// the outer wait loop (if any) continues iterating — these tests
/// drive `quiescence_classify_step` directly without an outer
/// [`agentmux::transports::wait_for_quiescent_three_state`] wrapper.
struct MockProbe {
    first_observation: WedgeObservation,
    later_observations: WedgeObservation,
    observe_count: u64,
}

impl MockProbe {
    fn new(first: WedgeObservation, later: WedgeObservation) -> Self {
        Self {
            first_observation: first,
            later_observations: later,
            observe_count: 0,
        }
    }
}

impl WedgeProbe for MockProbe {
    fn observe(&mut self) -> Result<WedgeObservation, String> {
        self.observe_count += 1;
        if self.observe_count == 1 {
            Ok(self.first_observation.clone())
        } else {
            Ok(self.later_observations.clone())
        }
    }
    fn wait_for_change(&mut self, _deadline: Instant) -> Result<(), DeliveryWaitError> {
        Ok(())
    }
}

/// Spec scenario: `Busy short-circuit suppresses wedged classification
/// on active target` (Tmux variant). When the activity signal advances
/// between two consecutive observation polls, the classifier must
/// suppress `wedged` classification and return `NeedsWait`.
#[test]
fn busy_short_circuit_returns_needs_wait_when_activity_advances() {
    // Probe simulates: at observation-before, activity=1, not ready;
    // at observation-after (1ms later), activity=2, still not ready.
    // The Busy short-circuit must fire and return `NeedsWait`.
    let mut probe = MockProbe::new(make_observation(1, false), make_observation(2, false));
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    assert!(
        matches!(result, QuiescenceAction::NeedsWait(_)),
        "expected NeedsWait after activity advance, got {result:?}",
    );
    // The wedge counter MUST have been reset to 0 by the Busy branch
    // (it was zero on entry; this confirms the reset code path
    // executed without asserting a specific numeric value).
    assert_eq!(
        state.consecutive_quiescent_mismatches(),
        0,
        "wedge counter must reset to 0 on Busy branch",
    );
}

/// Spec scenario: `Busy short-circuit defers Delivered during active
/// output (branch-ordering contract)`. The post-sleep observation is
/// prompt-ready AND activity advanced during the same quiet window.
/// The classifier MUST fire the Busy short-circuit (return
/// `NeedsWait`) rather than the `delivery_ready` branch (which would
/// return `Done(Ok(...))`).
///
/// This is the ordering-contract test: Busy short-circuit runs
/// BEFORE `delivery_ready`, not just before the wedge-counter
/// increment block. Without that ordering, the post-sleep
/// `is_prompt_ready == true` check would fire Delivered before Busy
/// had a chance to run.
#[test]
fn busy_short_circuit_defers_delivered_when_activity_advances_while_ready() {
    let mut probe = MockProbe::new(
        // Before: prompt-ready, gen=1.
        make_observation(1, true),
        // After: still prompt-ready, gen=2 (advanced).
        make_observation(2, true),
    );
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    assert!(
        matches!(result, QuiescenceAction::NeedsWait(_)),
        "expected NeedsWait when activity advances while ready (Busy short-circuit must win over delivery_ready), got {result:?}",
    );
}

/// Sanity baseline: with no activity advance and a prompt-ready
/// observation, the classifier resolves as `Done(Ok(...))`. This
/// verifies the new Busy short-circuit does not regress the
/// normal-flow path: a ready pane whose snapshot does not change
/// between observations and whose activity marker stays constant
/// still gets the `running` classification.
#[test]
fn delivery_ready_fires_when_ready_and_no_activity_advance() {
    let mut probe = MockProbe::new(
        make_observation(1, true),
        // Same activity generation, no advance.
        make_observation(1, true),
    );
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    assert!(
        matches!(result, QuiescenceAction::Done(Ok(ref pane)) if pane == TEST_PANE),
        "expected Done(Ok({TEST_PANE:?})) on prompt-ready stable observation, got {result:?}",
    );
}

/// The classifier has no branch that can indefinitely park a
/// prompt-ready pane. A prompt-ready observation resolves `Delivered`
/// on the first quiescent tick even under an unbounded prime deadline
/// (`prime_timeout_ms = None`) — the configuration that previously,
/// combined with an operator-copy-mode suppression branch, produced an
/// effectively-unbounded silent hang. There is no longer any observable
/// operator state a probe could report that would suppress this
/// delivery, so a pane scrolled into copy-mode still gets its message.
#[test]
fn prompt_ready_resolves_delivered_under_unbounded_prime() {
    let mut probe = MockProbe::new(make_observation(7, true), make_observation(7, true));
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    assert!(
        matches!(result, QuiescenceAction::Done(Ok(ref pane)) if pane == TEST_PANE),
        "prompt-ready pane must resolve Delivered with no suppression branch to park it, got {result:?}",
    );
}

/// Sanity baseline: when activity does NOT advance and the snapshot
/// is not prompt-ready (and the prime window is bounded short), the
/// prime-timeout branch fires. This confirms the Busy short-circuit
/// does not interfere with the regular Timeout path.
#[test]
fn prime_timeout_fires_when_not_ready_and_no_activity_advance() {
    let mut probe = MockProbe::new(make_observation(0, false), make_observation(0, false));
    let mut state = QuiescenceState::new();
    // Capture a started_at far enough in the past that prime_deadline
    // is already elapsed on the first call.
    let started_at = Instant::now() - Duration::from_secs(60);
    let prime_deadline = Some(started_at);
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &QuiescenceBounds {
            quiet_window: TEST_QUIET_WINDOW,
            started_at,
            prime_deadline,
            prime_timeout_ms: Some(0),
            readiness_deadline: None,
        },
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::Timeout { .. }))
        ),
        "expected Timeout when prime deadline elapsed with no activity, got {result:?}",
    );
}

/// Spec scenario: `Busy short-circuit resets wedge counter`. When the
/// wedge counter has accumulated across one or two quiesced iterations
/// AND the next observation reports an activity-signal advance, the
/// counter must reset to 0.
///
/// Test strategy: drive the counter to a nonzero value organically
/// via a wedge-class mismatch (non-empty `inspected_tail` + is_prompt_ready
/// = false; the cross-transport classifier's `resolve_mismatch_reason`
/// derives "prompt regex did not match" for that shape, which is
/// wedge-class). Then run a second classify_step on a FRESH probe
/// whose `activity_generation` advances between the two
/// `observe()` calls; the Busy short-circuit must reset the
/// counter to 0. The fresh probe is required because `MockProbe`
/// tracks `observe_count` and the second call would otherwise
/// return `later_observations` for both samples regardless of
/// `first_observation` mutation.
#[test]
fn busy_short_circuit_resets_wedge_counter() {
    fn wedge_class_observation(activity: u64) -> WedgeObservation {
        WedgeObservation {
            // Non-empty so the cross-transport classifier's
            // `resolve_mismatch_reason` resolves a wedge-class
            // reason (regex mismatch), not the empty-pane reason.
            // With an empty tail, the mismatch reason is the
            // `EMPTY_PANE_MISMATCH_PREFIX` and the counter never
            // increments — see `mismatch_is_wedge_class`.
            inspected_tail: "non-prompt screen content".to_string(),
            is_prompt_ready: false,
            pane_target: Some(TEST_PANE.to_string()),
            mismatch: None,
            activity_generation: activity,
        }
    }

    // Step 1: run classify_step on a wedge-class probe with
    // activity staying at 0. The counter increments to 1 because
    // the snapshot is non-quiesced-... actually quiesced (snap_before
    // == snap_after, both wedge-class), is_prompt_ready=false, and
    // `mismatch_is_wedge_class` returns true. This seeds the
    // counter to a known nonzero value so the Busy reset is
    // observable.
    let mut seed_probe = MockProbe::new(wedge_class_observation(0), wedge_class_observation(0));
    let mut state = QuiescenceState::new();
    let _ = quiescence_classify_step(
        &mut seed_probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(Instant::now(), None, None),
        true,
    );
    let counter_before_busy = state.consecutive_quiescent_mismatches();
    assert!(
        counter_before_busy > 0,
        "seed step should leave the wedge counter nonzero, got {counter_before_busy}",
    );

    // Step 2: a fresh probe whose `activity_generation` advances
    // between the two `observe()` calls (0 → 1). Busy fires and
    // resets the counter to 0. The fresh probe is required
    // because `MockProbe::observe` returns `first_observation`
    // only on the first call; reusing the seed probe from step
    // 1 (whose `observe_count` is already at 2) would return
    // `later_observations` for both samples and miss the Busy
    // branch.
    let mut busy_probe = MockProbe::new(wedge_class_observation(0), wedge_class_observation(1));
    let result = quiescence_classify_step(
        &mut busy_probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(Instant::now(), None, None),
        true,
    );
    assert!(
        matches!(result, QuiescenceAction::NeedsWait(_)),
        "expected NeedsWait after activity advance, got {result:?}",
    );
    assert_eq!(
        state.consecutive_quiescent_mismatches(),
        0,
        "wedge counter must reset to 0 after Busy short-circuit (was {counter_before_busy} before Busy, now {})",
        state.consecutive_quiescent_mismatches(),
    );
}

/// Cross-transport wait-loop contract: the Busy short-circuit's
/// `NeedsWait` must return a SHORT deadline (`~now + quiet_window`),
/// NOT the prime deadline (which can be unbounded when
/// `prime_timeout_ms = None`). Without this, the production wait
/// function can hang indefinitely after a Busy iteration: production
/// `wait_for_change` blocks until a transport change OR the deadline
/// elapses, and after activity settles there is no subsequent change
/// to wake the loop. With the bounded deadline, the wrapper
/// re-classifies within `quiet_window` and can fire `delivery_ready`
/// on the next iteration.
#[test]
fn busy_needswait_deadline_is_bounded_by_quiet_window() {
    // Probe returns two snapshots where activity advances and the
    // post-sleep observation is prompt-ready. The Busy short-circuit
    // must fire (activity advanced), then return `NeedsWait` with a
    // deadline near `now + quiet_window`.
    let mut probe = MockProbe::new(
        WedgeObservation {
            inspected_tail: String::new(),
            is_prompt_ready: false,
            pane_target: Some(TEST_PANE.to_string()),
            mismatch: None,
            activity_generation: 0,
        },
        WedgeObservation {
            inspected_tail: String::new(),
            is_prompt_ready: true,
            pane_target: Some(TEST_PANE.to_string()),
            mismatch: None,
            activity_generation: 1,
        },
    );
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    match result {
        QuiescenceAction::NeedsWait(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // `quiet_window` is the upper bound; allow up to 5x slack
            // for clock jitter on slow CI hosts. The pre-fix code
            // returned the unbounded deadline (~1 year from now),
            // which fails this assertion by orders of magnitude.
            assert!(
                remaining <= TEST_QUIET_WINDOW * 5,
                "Busy NeedsWait deadline must be bounded near quiet_window (= {TEST_QUIET_WINDOW:?}); got remaining {remaining:?} (would force wait_for_change to sleep ~1 year on unbounded prime)",
            );
        }
        other => panic!("expected NeedsWait after Busy, got {other:?}"),
    }
}

/// A stable wedge-class mismatch must keep advancing toward its terminal
/// verdict even when no prime timeout is configured. The production probe
/// otherwise waits for pane output that an idle pane will never produce.
#[test]
fn idle_wedge_rechecks_until_the_counter_fires() {
    let observation = WedgeObservation {
        inspected_tail: "idle non-prompt screen".to_string(),
        is_prompt_ready: false,
        pane_target: Some(TEST_PANE.to_string()),
        mismatch: None,
        activity_generation: 0,
    };
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    let started_at = Instant::now();

    for expected_count in 1..3 {
        let result = quiescence_classify_step(
            &mut probe,
            &mut state,
            &diagnostic_context(),
            &quiescence_bounds(started_at, None, None),
            true,
        );
        let QuiescenceAction::NeedsWait(deadline) = result else {
            panic!("idle wedge must await another observation before threshold");
        };
        assert_eq!(state.consecutive_quiescent_mismatches(), expected_count);
        assert!(
            deadline.saturating_duration_since(Instant::now()) <= TEST_QUIET_WINDOW,
            "idle wedge recheck must not use the one-year sentinel",
        );
    }

    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &quiescence_bounds(started_at, None, None),
        true,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::Wedged { .. }))
        ),
        "third stable observation must classify the idle pane as wedged: {result:?}",
    );
}

/// A matching prompt frame with a non-idle cursor means the operator has
/// composed input and paused. Stable pending input must not become a wedge
/// merely because the idle-pane fix now rechecks genuine regex mismatches.
#[test]
fn composing_cursor_mismatch_never_advances_the_wedge_counter() {
    let observation = WedgeObservation {
        inspected_tail: "prompt frame with pending input".to_string(),
        is_prompt_ready: false,
        pane_target: Some(TEST_PANE.to_string()),
        mismatch: Some(ReadinessMismatch {
            reason: "cursor column 12 did not match required 5".to_string(),
            regex_matched: Some(true),
            expected_cursor_column: Some(5),
            observed_cursor_column: Some(12),
        }),
        activity_generation: 0,
    };
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();

    for _ in 0..4 {
        let result = quiescence_classify_step(
            &mut probe,
            &mut state,
            &diagnostic_context(),
            &quiescence_bounds(Instant::now(), None, None),
            true,
        );
        assert!(
            matches!(result, QuiescenceAction::NeedsWait(_)),
            "composing pane must remain pending rather than wedge: {result:?}",
        );
        assert_eq!(state.consecutive_quiescent_mismatches(), 0);
    }
}

// ---------------------------------------------------------------------------
// Readiness bound
// ---------------------------------------------------------------------------

/// Builds an observation with an explicit tail and optional cursor-class
/// mismatch, so the reason classifier's four arms can be driven apart.
fn readiness_observation(
    activity_generation: u64,
    inspected_tail: &str,
    regex_matched: Option<bool>,
) -> WedgeObservation {
    WedgeObservation {
        inspected_tail: inspected_tail.to_string(),
        is_prompt_ready: false,
        pane_target: Some(TEST_PANE.to_string()),
        mismatch: regex_matched.map(|matched| ReadinessMismatch {
            reason: "scripted mismatch".to_string(),
            regex_matched: Some(matched),
            expected_cursor_column: Some(4),
            observed_cursor_column: Some(0),
        }),
        activity_generation,
    }
}

/// A bound anchored far enough in the past that it has already elapsed.
fn elapsed_readiness_bounds() -> QuiescenceBounds {
    QuiescenceBounds {
        quiet_window: TEST_QUIET_WINDOW,
        started_at: Instant::now() - Duration::from_secs(120),
        prime_deadline: None,
        prime_timeout_ms: None,
        readiness_deadline: Some(Instant::now() - Duration::from_secs(60)),
    }
}

/// Task 4.1 — the Band 2 defect itself. A target whose activity advances on
/// every observation was previously suppressed indefinitely by the Busy
/// short-circuit; the readiness bound must end the wait.
#[test]
fn advancing_activity_terminates_at_the_readiness_bound() {
    let mut probe = MockProbe::new(
        readiness_observation(1, "still working", None),
        readiness_observation(2, "still working", None),
    );
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &elapsed_readiness_bounds(),
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout {
                reason_code: ReadinessTimeoutReason::TargetNeverSettled,
                ..
            }))
        ),
        "expected ReadinessTimeout/target_never_settled, got {result:?}",
    );
}

/// Task 4.2 — each reason-classifier arm at expiry, and none of them a
/// `pane_wedged` failure.
#[test]
fn readiness_expiry_reports_the_reason_for_each_observation_shape() {
    let cases = [
        (
            readiness_observation(7, "", None),
            ReadinessTimeoutReason::TargetUnresponsive,
        ),
        (
            readiness_observation(7, "$ ", Some(true)),
            ReadinessTimeoutReason::PendingOperatorInput,
        ),
        (
            readiness_observation(7, "a dialog", Some(false)),
            ReadinessTimeoutReason::TargetNotReady,
        ),
    ];
    for (observation, expected) in cases {
        let mut probe = MockProbe::new(observation.clone(), observation.clone());
        let mut state = QuiescenceState::new();
        let result = quiescence_classify_step(
            &mut probe,
            &mut state,
            &diagnostic_context(),
            &elapsed_readiness_bounds(),
            false,
        );
        match result {
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout {
                reason_code,
                ..
            })) => assert_eq!(reason_code, expected, "for observation {observation:?}"),
            other => panic!("expected ReadinessTimeout for {observation:?}, got {other:?}"),
        }
    }
}

/// Task 4.3 — reason precedence: activity outranks every static signal,
/// because it is the only one describing the observation *pair*.
#[test]
fn activity_outranks_the_static_reason_signals() {
    // Empty tail AND a cursor-class mismatch would each claim a reason, but
    // the activity advance describes the pair and wins.
    let reason = classify_readiness_timeout_reason(
        &readiness_observation(1, "", Some(true)),
        &readiness_observation(2, "", Some(true)),
    );
    assert_eq!(reason, ReadinessTimeoutReason::TargetNeverSettled);

    // With activity settled, the empty tail outranks the cursor mismatch.
    let reason = classify_readiness_timeout_reason(
        &readiness_observation(2, "", Some(true)),
        &readiness_observation(2, "", Some(true)),
    );
    assert_eq!(reason, ReadinessTimeoutReason::TargetUnresponsive);
}

/// Task 4.4 — the prime timeout outranks the readiness bound when both have
/// elapsed in the same iteration: it is the more specific diagnosis and the
/// operator opted into it.
#[test]
fn prime_timeout_outranks_a_simultaneously_elapsed_readiness_bound() {
    let long_ago = Instant::now() - Duration::from_secs(120);
    let bounds = QuiescenceBounds {
        quiet_window: TEST_QUIET_WINDOW,
        started_at: long_ago,
        prime_deadline: Some(long_ago),
        prime_timeout_ms: Some(1),
        readiness_deadline: Some(long_ago),
    };
    let observation = readiness_observation(7, "", None);
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::Timeout { .. }))
        ),
        "expected the prime Timeout to win, got {result:?}",
    );

    // Discriminating half: the same elapsed bound with no prime timeout
    // configured resolves as the readiness expiry. Without this the test
    // would pass against a build that ignores the readiness bound entirely,
    // because the prime timeout fires either way.
    let without_prime = QuiescenceBounds {
        prime_deadline: None,
        prime_timeout_ms: None,
        ..bounds
    };
    let observation = readiness_observation(7, "", None);
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &without_prime,
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout { .. }))
        ),
        "the bound must resolve the group when no prime timeout outranks it, got {result:?}",
    );
}

/// Task 4.9 — delivery outranks a simultaneous expiry. Reaching readiness
/// late is the outcome the wait existed to obtain.
#[test]
fn delivery_outranks_a_simultaneously_elapsed_readiness_bound() {
    let ready = WedgeObservation {
        is_prompt_ready: true,
        ..readiness_observation(7, "$ ", None)
    };
    let mut probe = MockProbe::new(ready.clone(), ready);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &elapsed_readiness_bounds(),
        false,
    );
    assert!(
        matches!(result, QuiescenceAction::Done(Ok(_))),
        "expected Delivered despite the elapsed bound, got {result:?}",
    );

    // Discriminating half: the identical bound resolves a target that is NOT
    // ready. Without this the test would pass against a build that never
    // consults the bound, since delivery fires either way.
    let unready = readiness_observation(7, "a dialog", Some(false));
    let mut probe = MockProbe::new(unready.clone(), unready);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &elapsed_readiness_bounds(),
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout { .. }))
        ),
        "the same elapsed bound must resolve an unready target, got {result:?}",
    );
}

/// Task 4.9, second half — a Busy target that merely *looks* ready in the
/// iteration the bound elapses is not granted the match Busy just denied.
#[test]
fn busy_target_is_not_delivered_on_a_momentary_match_at_expiry() {
    let ready_before = WedgeObservation {
        is_prompt_ready: true,
        ..readiness_observation(1, "$ ", None)
    };
    let ready_after = WedgeObservation {
        is_prompt_ready: true,
        ..readiness_observation(2, "$ ", None)
    };
    let mut probe = MockProbe::new(ready_before, ready_after);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &elapsed_readiness_bounds(),
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout {
                reason_code: ReadinessTimeoutReason::TargetNeverSettled,
                ..
            }))
        ),
        "expected target_never_settled rather than a granted momentary match, got {result:?}",
    );
}

/// Tasks 4.6 and 4.7 — an absent prime timeout still terminates on the bound,
/// and no `NeedsWait` deadline the classifier returns may exceed it.
#[test]
fn no_scheduled_wait_outlives_the_readiness_bound() {
    // No prime deadline, so the classifier's fall-through would otherwise
    // propose the one-year `unbounded_deadline`. The bound must shorten it.
    let started_at = Instant::now();
    let bound = started_at + Duration::from_secs(5);
    let bounds = QuiescenceBounds {
        quiet_window: TEST_QUIET_WINDOW,
        started_at,
        prime_deadline: None,
        prime_timeout_ms: None,
        readiness_deadline: Some(bound),
    };
    let observation = readiness_observation(7, "a dialog", Some(false));
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    match quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    ) {
        QuiescenceAction::NeedsWait(deadline) => assert!(
            deadline <= bound,
            "a scheduled wait outlived the readiness bound",
        ),
        QuiescenceAction::Done(result) => {
            panic!("expected NeedsWait before the bound, got {result:?}")
        }
    }
}

/// AuxBE `reviews/relay/13` finding 2 — removing wedge detection must not widen
/// the prime timeout's scope.
///
/// The prime timeout answers "the target never produced observable output". A
/// settled wedge-class frame is a target that *answered* — a permission dialog,
/// a compose box — and used to be intercepted by the wedge branch before it
/// could reach the prime branch. With wedge detection off for Tmux that
/// interception is gone, so a settled dialog would fall through and terminate on
/// the prime timeout: the same inference from absence the wedge classifier was
/// removed for making, wearing a different reason code.
///
/// Both halves are asserted together, because the fix is a narrowing and a test
/// that only proves the narrowing would also pass if the prime timeout were
/// disabled outright.
#[test]
fn an_elapsed_prime_timeout_spares_a_settled_frame_but_not_a_silent_target() {
    // Prime elapsed 60s ago; the readiness bound is still live.
    let bounds = QuiescenceBounds {
        quiet_window: TEST_QUIET_WINDOW,
        started_at: Instant::now() - Duration::from_secs(120),
        prime_deadline: Some(Instant::now() - Duration::from_secs(60)),
        prime_timeout_ms: Some(60_000),
        readiness_deadline: Some(Instant::now() + Duration::from_secs(600)),
    };

    // A settled permission dialog: non-empty tail, regex did not match.
    let dialog = readiness_observation(7, "Do you want to allow this? (y/n)", Some(false));
    let mut probe = MockProbe::new(dialog.clone(), dialog);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    );
    assert!(
        matches!(result, QuiescenceAction::NeedsWait(_)),
        "an elapsed prime timeout must not adjudicate a settled frame; got {result:?}",
    );

    // A target that produced nothing at all is what the prime timeout is for.
    let silent = make_observation(7, false);
    let mut probe = MockProbe::new(silent.clone(), silent);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    );
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::Timeout { .. }))
        ),
        "the prime timeout must still fire for a target with no observable output; got {result:?}",
    );
}

/// AuxBE `reviews/relay/13` finding 1 — the bound must not overshoot by a full
/// quiet window.
///
/// Capping the `NeedsWait` deadlines makes the wrapper wake at the bound, but
/// the classifier then observes, sleeps a quiet window, observes, and only then
/// evaluates the bound. An uncapped sleep therefore reports expiry a whole
/// window late and opens a post-deadline interval in which a target that was
/// not ready at the bound can still deliver.
///
/// The quiet window here (2s) is far longer than any real one so the overshoot
/// would be unmistakable; the assertion allows an order of magnitude of
/// scheduling slack and still fails against the unshortened sleep.
#[test]
fn an_elapsed_bound_is_not_slept_past() {
    let observation = readiness_observation(7, "a dialog", Some(false));
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    let bounds = QuiescenceBounds {
        quiet_window: Duration::from_secs(2),
        started_at: Instant::now() - Duration::from_secs(120),
        prime_deadline: None,
        prime_timeout_ms: None,
        readiness_deadline: Some(Instant::now() - Duration::from_secs(60)),
    };
    let entered_at = Instant::now();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    );
    let spent = entered_at.elapsed();
    assert!(
        matches!(
            result,
            QuiescenceAction::Done(Err(DeliveryWaitError::ReadinessTimeout { .. }))
        ),
        "expected the elapsed bound to resolve, got {result:?}",
    );
    assert!(
        spent < Duration::from_millis(500),
        "an iteration entered past the bound must not sleep another quiet window; \
         spent {spent:?} of a 2s window",
    );
}

/// Task 4.5 — a settled Tmux target whose readiness frame is absent must
/// produce no terminal outcome until the bound elapses, whatever the target
/// shows. This is the whole point of dropping wedge detection from Tmux: the
/// mismatch counter still accumulates past its threshold (asserted below, so
/// the test cannot pass merely because the observations were never wedge-class),
/// but with `wedge_detection: false` the counter fires nothing. A settled
/// non-prompt frame is produced by a hung coder, a permission dialog, a compose
/// box, and a coder working silently alike, and only the bound may end the wait.
#[test]
fn a_settled_absent_frame_produces_no_outcome_before_the_bound() {
    // A live bound, far enough out that none of the iterations below reach it.
    let bounds = quiescence_bounds(Instant::now(), None, Some(60_000));
    let contents = [
        "Do you want to allow this? (y/n)",
        "",
        "$ half-typed command",
        "\u{2588}",
    ];
    for content in contents {
        let observation = readiness_observation(7, content, Some(false));
        let mut probe = MockProbe::new(observation.clone(), observation);
        let mut state = QuiescenceState::new();
        for iteration in 0..=WEDGE_CONSECUTIVE_TICKS {
            let result = quiescence_classify_step(
                &mut probe,
                &mut state,
                &diagnostic_context(),
                &bounds,
                false,
            );
            assert!(
                matches!(result, QuiescenceAction::NeedsWait(_)),
                "iteration {iteration} on {content:?} must keep waiting, got {result:?}",
            );
        }
        assert!(
            state.consecutive_quiescent_mismatches() > WEDGE_CONSECUTIVE_TICKS,
            "the observations must be wedge-class for this test to discriminate; \
             counter reached {} on {content:?}",
            state.consecutive_quiescent_mismatches(),
        );
    }
}

/// Task 4.8 — the shared code path must not impose the bound on a transport
/// that carries none. This is the guard that keeps Pty's behavior identical.
#[test]
fn a_group_without_a_readiness_bound_is_unaffected() {
    let started_at = Instant::now() - Duration::from_secs(120);
    let bounds = QuiescenceBounds::new(TEST_QUIET_WINDOW, started_at, None, None);
    let observation = readiness_observation(7, "a dialog", Some(false));
    let mut probe = MockProbe::new(observation.clone(), observation);
    let mut state = QuiescenceState::new();
    let result = quiescence_classify_step(
        &mut probe,
        &mut state,
        &diagnostic_context(),
        &bounds,
        false,
    );
    assert!(
        matches!(result, QuiescenceAction::NeedsWait(_)),
        "a group with no bound must keep waiting exactly as before, got {result:?}",
    );
}

/// Task 4.12 — both deadlines share one anchor, so a later envelope absorbed
/// by coalesce-during-wait cannot shift either.
#[test]
fn both_bounds_derive_from_one_anchor() {
    let started_at = Instant::now();
    let bounds = QuiescenceBounds::new(TEST_QUIET_WINDOW, started_at, Some(1_000), Some(5_000));
    assert_eq!(
        bounds.prime_deadline,
        Some(started_at + Duration::from_millis(1_000)),
    );
    assert_eq!(
        bounds.readiness_deadline,
        Some(started_at + Duration::from_millis(5_000)),
    );
    assert_eq!(bounds.started_at, started_at);
}

/// Builds an envelope carrying the quiescence hints a flush group anchors on.
fn hinted_envelope(
    message_id: &str,
    quiet_window: Duration,
    prime_timeout_ms: Option<u64>,
    readiness_timeout_ms: Option<u64>,
) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: message_id.to_string(),
        message: DeliveryMessage {
            body: "body".to_string(),
            created_at: "2026-08-01T00:00:00Z".to_string(),
            namespace: "test-namespace".to_string(),
            sender: AddressIdentity {
                session_name: "sender".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: TEST_TARGET_SESSION.to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window,
        prime_timeout_ms,
        readiness_timeout_ms,
        is_receipt: false,
    }
}

/// Task 4.12, production shape — the head envelope owns the group's bounds and
/// a later one absorbed by coalesce-during-wait cannot shift them.
///
/// The anchoring rule used to live as a bare `group[0]` index at the Tmux call
/// site, where nothing could observe it: every later envelope's hints were
/// discarded by an expression, not by a rule. Deriving the bounds from the whole
/// group and discarding the tail makes head-ownership the function's behavior,
/// so this test can state it. Every later hint here differs from the head's in
/// both directions, so a `last()`, a `min`, or a `max` would each be caught.
#[test]
fn a_groups_bounds_come_from_its_head_envelope_only() {
    let started_at = Instant::now();
    let group = [
        hinted_envelope("head", TEST_QUIET_WINDOW, Some(1_000), Some(5_000)),
        hinted_envelope("later-shorter", Duration::from_secs(9), Some(10), Some(30)),
        hinted_envelope(
            "later-longer",
            Duration::from_secs(99),
            Some(900_000),
            Some(3_600_000),
        ),
    ];
    let bounds = QuiescenceBounds::from_group(group.iter(), started_at)
        .expect("a non-empty group has a head");

    assert_eq!(bounds.quiet_window, TEST_QUIET_WINDOW);
    assert_eq!(bounds.prime_timeout_ms, Some(1_000));
    assert_eq!(
        bounds.prime_deadline,
        Some(started_at + Duration::from_millis(1_000)),
    );
    assert_eq!(
        bounds.readiness_deadline,
        Some(started_at + Duration::from_millis(5_000)),
    );

    // A group with nothing in it has no head to anchor to, which is how the
    // production call site subsumes its own emptiness check.
    assert!(QuiescenceBounds::from_group(&[], started_at).is_none());
}
