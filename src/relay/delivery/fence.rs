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
    let mut fence = FenceInProgress::begin(generation, observation);
    let interval = Duration::from_millis(OBSERVATION_POLL_INTERVAL_MS);
    loop {
        if let Some(outcome) = fence.advance(generation, Instant::now()) {
            return outcome;
        }
        tokio::time::sleep(interval).await;
    }
}

/// A fence acknowledgment in progress, advanced by its owner rather than
/// awaited.
///
/// The step-driven shape exists because unit evidence stays admissible through
/// both observation windows: a caller that awaited the fence as one future would
/// stop collecting the very outcomes the fence is supposed to let it keep
/// collecting. [`acknowledge_fence`] is the thin awaiting driver over this, so
/// there is exactly one implementation of the protocol.
#[derive(Debug)]
pub struct FenceInProgress {
    observation: Duration,
    /// End of the window currently being observed.
    deadline: Instant,
    /// Whether forced termination has already been invoked, which is what
    /// distinguishes the first window from the second.
    escalated: bool,
}

impl FenceInProgress {
    /// Performs step 1 — the cooperative stop request — and opens the first
    /// observation window.
    pub fn begin<G>(generation: &mut G, observation: Duration) -> Self
    where
        G: GenerationFence + ?Sized,
    {
        generation.fence_generation();
        Self {
            observation,
            deadline: Instant::now() + observation,
            escalated: false,
        }
    }

    /// Advances the fence, returning its outcome once one is established.
    ///
    /// `None` means the fence is still observing and the caller should keep
    /// doing its own work — collecting outcomes above all — and call again.
    /// Observing is a poll rather than a join because no runtime primitive can
    /// force a thread blocked in a syscall to return.
    pub fn advance<G>(&mut self, generation: &mut G, now: Instant) -> Option<FenceOutcome>
    where
        G: GenerationFence + ?Sized,
    {
        if generation.generation_ceased() {
            return Some(FenceOutcome {
                verdict: FenceVerdict::Positive,
                resolution: if self.escalated {
                    FenceResolution::Forced
                } else {
                    FenceResolution::Cooperative
                },
            });
        }
        if now < self.deadline {
            return None;
        }
        if self.escalated {
            return Some(FenceOutcome {
                verdict: FenceVerdict::Negative,
                resolution: FenceResolution::Unobserved,
            });
        }
        // The first window elapsed without cessation. The primitive initiates
        // and returns; its success acknowledges nothing. It exists so the second
        // window's observation can succeed where the first could not.
        generation.terminate_generation();
        self.escalated = true;
        self.deadline = now + self.observation;
        None
    }
}
