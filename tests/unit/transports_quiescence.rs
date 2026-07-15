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

use agentmux::transports::{
    DeliveryWaitError, QuiescenceAction, QuiescenceState, WedgeObservation, WedgeProbe,
    quiescence_classify_step,
};

const TEST_TARGET_SESSION: &str = "test-session";
const TEST_QUIET_WINDOW: Duration = Duration::from_millis(1);
const TEST_PANE: &str = "%0";

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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        // Unbounded prime deadline so the prime-timeout branch cannot
        // fire and confuse the test.
        None,
        started_at,
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        None,
        started_at,
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        None,
        started_at,
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        // Unbounded: the exact deadline that paired with the removed
        // copy-mode gate to hang forever.
        None,
        started_at,
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        prime_deadline,
        started_at,
        Some(0),
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        None,
        Instant::now(),
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        None,
        Instant::now(),
        None,
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
        TEST_TARGET_SESSION,
        TEST_QUIET_WINDOW,
        // Unbounded prime deadline: under the pre-fix code the
        // NeedsWait deadline would be ~now + 1 year; the test asserts
        // the deadline is bounded near `quiet_window` instead.
        None,
        started_at,
        None,
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
