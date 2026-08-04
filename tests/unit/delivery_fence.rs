//! The five-step generation fence protocol.
//!
//! Driven against a controllable generation rather than a live transport,
//! because what is under test is the protocol's *ordering and bounds* — which
//! step ran, which did not, and what the verdict was — and no real transport
//! lets a test hold an executor un-ceased on demand.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agentmux::relay::{FenceResolution, FenceVerdict, acknowledge_fence};
use agentmux::transports::GenerationFence;

/// A generation whose cessation the test controls directly.
///
/// `ceases_on_fence` models an executor that checks the cooperative flag;
/// `ceases_on_terminate` models one that only stops when its effect path is torn
/// out from under it. A generation with neither never ceases, which is the
/// fail-stop case.
struct ControllableGeneration {
    ceases_on_fence: bool,
    ceases_on_terminate: bool,
    ceased: Arc<AtomicBool>,
    fence_calls: Arc<AtomicUsize>,
    terminate_calls: Arc<AtomicUsize>,
}

impl ControllableGeneration {
    fn new(ceases_on_fence: bool, ceases_on_terminate: bool) -> Self {
        Self {
            ceases_on_fence,
            ceases_on_terminate,
            ceased: Arc::new(AtomicBool::new(false)),
            fence_calls: Arc::new(AtomicUsize::new(0)),
            terminate_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl GenerationFence for ControllableGeneration {
    fn fence_generation(&mut self) {
        self.fence_calls.fetch_add(1, Ordering::SeqCst);
        if self.ceases_on_fence {
            self.ceased.store(true, Ordering::SeqCst);
        }
    }

    fn terminate_generation(&mut self) {
        self.terminate_calls.fetch_add(1, Ordering::SeqCst);
        if self.ceases_on_terminate {
            self.ceased.store(true, Ordering::SeqCst);
        }
    }

    fn generation_ceased(&self) -> bool {
        self.ceased.load(Ordering::SeqCst)
    }
}

/// Forced termination is destructive — it tears down a child or revokes a
/// broadcaster — so an executor that stops when asked must never reach it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cooperative_stop_never_invokes_forced_termination() {
    let mut generation = ControllableGeneration::new(true, false);
    let terminate_calls = Arc::clone(&generation.terminate_calls);

    let outcome = acknowledge_fence(&mut generation, Duration::from_millis(200)).await;

    assert_eq!(outcome.verdict, FenceVerdict::Positive);
    assert_eq!(outcome.resolution, FenceResolution::Cooperative);
    assert_eq!(
        terminate_calls.load(Ordering::SeqCst),
        0,
        "a generation that ceased cooperatively must not be force-terminated"
    );
}

/// The escalation path: the cooperative flag reaches nothing, and only tearing
/// out the effect path lets the second observation succeed where the first
/// could not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresponsive_generation_escalates_and_then_ceases() {
    let mut generation = ControllableGeneration::new(false, true);
    let fence_calls = Arc::clone(&generation.fence_calls);
    let terminate_calls = Arc::clone(&generation.terminate_calls);

    let outcome = acknowledge_fence(&mut generation, Duration::from_millis(200)).await;

    assert_eq!(outcome.verdict, FenceVerdict::Positive);
    assert_eq!(outcome.resolution, FenceResolution::Forced);
    assert_eq!(
        fence_calls.load(Ordering::SeqCst),
        1,
        "the cooperative request is still made first"
    );
    assert_eq!(terminate_calls.load(Ordering::SeqCst), 1);
}

/// Fail-stop. Timeout and failure both route to a negative verdict; there is no
/// third outcome, because a target whose old generation might still write
/// alongside a new one is not recoverable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_generation_that_never_ceases_is_fenced_negative() {
    let mut generation = ControllableGeneration::new(false, false);

    let outcome = acknowledge_fence(&mut generation, Duration::from_millis(100)).await;

    assert_eq!(outcome.verdict, FenceVerdict::Negative);
    assert_eq!(outcome.resolution, FenceResolution::Unobserved);
}

/// Acknowledgment is bounded end to end, not only before escalation.
///
/// Steps 1 and 3 are signals that consume none of the budget, so a generation
/// that never ceases costs two observation windows and no more. Without the
/// second window being bounded — or with either observation implemented as a
/// blocking join — this is where the unbounded wait would reappear.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledgment_is_bounded_by_twice_the_observation_window() {
    let mut generation = ControllableGeneration::new(false, false);
    let observation = Duration::from_millis(150);

    let started = Instant::now();
    let outcome = acknowledge_fence(&mut generation, observation).await;
    let elapsed = started.elapsed();

    assert_eq!(outcome.verdict, FenceVerdict::Negative);
    assert!(
        elapsed >= observation * 2,
        "both windows must actually be observed, not short-circuited: {elapsed:?}"
    );
    // Generous headroom over 2x: this asserts the absence of a *third* window or
    // an unbounded join, not scheduler precision.
    assert!(
        elapsed < observation * 4,
        "acknowledgment must stay bounded by twice the window: {elapsed:?}"
    );
}

/// A generation that owns nothing running is already ceased, so the fence costs
/// no wall time. This is the common case — most fences find nothing executing —
/// and paying an observation window for it would put the fence's cost on every
/// teardown rather than only on the ones that need it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_already_ceased_generation_costs_no_observation_time() {
    let mut generation = ControllableGeneration::new(false, false);
    generation.ceased.store(true, Ordering::SeqCst);

    let started = Instant::now();
    let outcome = acknowledge_fence(&mut generation, Duration::from_secs(30)).await;

    assert_eq!(outcome.verdict, FenceVerdict::Positive);
    assert_eq!(outcome.resolution, FenceResolution::Cooperative);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an already-ceased generation must not wait out its observation window"
    );
}
