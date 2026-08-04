//! The generation fence: the five-step protocol that establishes whether a
//! transport generation has stopped executing.
//!
//! This answers one question and only one — *has execution ceased?* — which is
//! deliberately not the same question as *has this member resolved?* Those are
//! separate facts with separate consequences:
//!
//! | Fact | Established by | Releases |
//! |---|---|---|
//! | Outcome terminal | the guard's terminal transition | admission quota, receipts, outcome-level barriers |
//! | Execution ceased | fence acknowledgment | nothing on its own |
//! | Target-side ordering safe | execution ceased, and no in-flight primitive can still take effect | the raw barrier |
//!
//! So a member may terminalize `submission_unknown` long before its generation
//! is fenced — that is the honest report of an unknown — while replacement and
//! the ordering barriers stay held until the fence is positive. A negative
//! verdict strands no message; it stalls one target's lifecycle.

use std::time::{Duration, Instant};

use crate::transports::GenerationFence;

/// How often cessation is re-observed inside a bounded observation window.
///
/// The window's duration is the contract; this is only the granularity at which
/// it is checked. Small enough that a cooperative stop is noticed promptly,
/// large enough that observing costs nothing measurable against a multi-second
/// window.
const OBSERVATION_POLL_INTERVAL_MS: u64 = 10;

/// The result of fence acknowledgment. There is no third outcome: timeout and
/// failure both route to [`Negative`](Self::Negative).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceVerdict {
    /// Every generation-owned executor was observed to cease. Replacement and
    /// the target's ordering barriers may proceed.
    Positive,
    /// Cessation was not observed within the budget. The supervisor admits no
    /// replacement for this target and holds its raw barrier.
    ///
    /// Deliberately fail-stop: a target that stops accepting new generations is
    /// recoverable by operator action, and a target whose old generation might
    /// still write alongside a new one is not.
    Negative,
}

/// Which step produced the verdict, for observability. The distinction matters
/// operationally: a fence that needed forced termination says something
/// different about a transport than one that stopped cooperatively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceResolution {
    /// Ceased in the first window; the termination primitive was never invoked.
    Cooperative,
    /// Ceased in the second window, after forced termination.
    Forced,
    /// Never observed to cease.
    Unobserved,
}

/// The outcome of one fence acknowledgment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceOutcome {
    pub verdict: FenceVerdict,
    pub resolution: FenceResolution,
}

/// Runs the five-step fence acknowledgment against one generation.
///
/// 1. cooperative stop request — non-blocking;
/// 2. first bounded cessation observation;
/// 3. forced generation termination — non-blocking;
/// 4. second bounded cessation observation;
/// 5. verdict.
///
/// Steps 1 and 3 are signals and consume none of the budget, so total
/// acknowledgment is bounded by twice `observation`. Steps 1 and 3 are kept
/// distinct on purpose: step 1 asks an executor that *can* observe a flag to
/// stop, and step 3 is the destructive action for one that cannot. Collapsing
/// them would force-terminate executors that were about to stop by themselves.
pub async fn acknowledge_fence<G>(generation: &mut G, observation: Duration) -> FenceOutcome
where
    G: GenerationFence + ?Sized,
{
    generation.fence_generation();
    if observe_cessation(generation, observation).await {
        return FenceOutcome {
            verdict: FenceVerdict::Positive,
            resolution: FenceResolution::Cooperative,
        };
    }

    // The primitive initiates and returns. Its success is not an
    // acknowledgment — it exists so the observation below can succeed where the
    // one above could not.
    generation.terminate_generation();
    if observe_cessation(generation, observation).await {
        return FenceOutcome {
            verdict: FenceVerdict::Positive,
            resolution: FenceResolution::Forced,
        };
    }

    FenceOutcome {
        verdict: FenceVerdict::Negative,
        resolution: FenceResolution::Unobserved,
    }
}

/// Observes for cessation for up to `budget`, polling rather than joining.
///
/// Checks once before sleeping so a generation that has already ceased costs no
/// wall time — which is what keeps the common case, where nothing was executing
/// in the first place, from paying an observation window it does not need.
async fn observe_cessation<G>(generation: &G, budget: Duration) -> bool
where
    G: GenerationFence + ?Sized,
{
    let deadline = Instant::now() + budget;
    let interval = Duration::from_millis(OBSERVATION_POLL_INTERVAL_MS);
    loop {
        if generation.generation_ceased() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(interval.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
}
