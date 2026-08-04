//! Queue entry state, delivery identities, and the relay-owned authorization
//! guard's evidence order.
//!
//! The types here describe *what a queue entry is* and *how a member resolves
//! when nothing told it what happened*. The ledger that holds them — and the
//! single lock under which the terminal transition and the quota release happen
//! together — lives in [`super::admission`], because releasing quota anywhere
//! other than the terminal transition is the leak this whole model exists to
//! prevent.
//!
//! ## Why the guard is relay-owned
//!
//! A keyed map plus a compare-and-swap is not, on its own, enough to make
//! exactly-once resolution a property: it cannot observe a detached thread, a
//! panicked worker or collector task, or a generation replacement. Those are
//! *triggers*. What makes them safe is that none of them chooses an outcome —
//! they all route through one evidence order, so a member the relay can
//! positively prove was never handed to a transport resolves `not_submitted`
//! rather than being smeared into `submission_unknown` by whichever lifecycle
//! event happened to fire.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::transports::SendOutcome;

/// Where a queue entry sits in the delivery lifecycle.
///
/// The three states are ordered and the transitions are one-way. `Pending` is
/// unbounded by design — an entry waits as long as its target takes, because
/// elapsed time spent waiting for readiness is not evidence about the target —
/// and it holds nothing but its own admission reservation. `Authorized` is the
/// state that holds a target's ordering position, so it is the side that carries
/// a bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum QueueEntryState {
    /// Admitted and waiting. Leaves this state only by authorization, positively
    /// observed transport teardown, or graceful shutdown — never by elapsed time.
    Pending,
    /// Authorization has irrevocably transferred delivery responsibility. The
    /// relay never reclaims, never retries, and never asserts non-delivery by
    /// inference from here.
    Authorized,
    /// Resolved exactly once. Admission quota is released on entry to this state
    /// and nowhere else.
    Terminal,
}

/// Typed evidence about whether a member's submission produced a target-side
/// effect.
///
/// An undifferentiated error maps to [`SubmissionUnknown`](Self::SubmissionUnknown),
/// never [`NotSubmitted`](Self::NotSubmitted). Only a primitive that can prove
/// nothing was written may claim the latter: a Tmux paste is a body write
/// followed by an Enter, and a Pty unit is several `write_all` calls, so both can
/// fail *after* partial effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum SubmissionEvidence {
    /// The target-side primitive positively reported success.
    Submitted,
    /// Positive evidence that no side effect occurred.
    NotSubmitted,
    /// Side effects cannot be excluded.
    SubmissionUnknown,
}

impl SubmissionEvidence {
    /// The relay outcome vocabulary this evidence resolves to.
    ///
    /// `SubmissionUnknown` deliberately has no failure spelling: not knowing is
    /// what actually happened, and reporting a failure would assert non-delivery
    /// the relay cannot support.
    pub(in crate::relay) fn outcome(self) -> SendOutcome {
        match self {
            Self::Submitted => SendOutcome::Delivered,
            Self::NotSubmitted => SendOutcome::NotSubmitted,
            Self::SubmissionUnknown => SendOutcome::SubmissionUnknown,
        }
    }

    /// The stable reason code carried on a receipt and in inscriptions.
    pub(in crate::relay) fn reason_code(self) -> &'static str {
        match self {
            Self::Submitted => "delivered",
            Self::NotSubmitted => "not_submitted",
            Self::SubmissionUnknown => "submission_unknown",
        }
    }
}

/// A lifecycle event that terminalizes a guard without choosing its outcome.
///
/// Every variant here is a *trigger*: it says the member can no longer be
/// resolved by the path that owned it, not what happened to the member. The
/// outcome comes from [`resolve_from_evidence`] in every case, which is what
/// keeps a positively-provable `not_submitted` from being reported as
/// `submission_unknown` merely because a task panicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum GuardTrigger {
    /// The collector task carrying this member panicked, taking the member's
    /// outcome with it.
    CollectorPanic,
    /// The transport's outcome channel closed before resolving.
    ChannelClosed,
    /// The relay is shutting down gracefully.
    GracefulShutdown,
}

impl GuardTrigger {
    /// The reason string recorded alongside the resolved outcome, naming the
    /// trigger rather than restating the evidence.
    pub(in crate::relay) fn reason(self) -> &'static str {
        match self {
            Self::CollectorPanic => "delivery collector task panicked before resolving",
            Self::ChannelClosed => "transport outcome channel closed before resolving",
            Self::GracefulShutdown => "relay shut down before this member resolved",
        }
    }
}

/// The guard's single evidence order, applied whenever a trigger fires with no
/// outcome of its own.
///
/// The order is deliberate and is the whole reason lifecycle events are not
/// allowed to pick a spelling:
///
/// 1. a recorded unit result, if the member's packing unit produced one;
/// 2. otherwise `not_submitted`, when the member was never handed to a
///    transport — nothing could have taken effect, so this is provable rather
///    than inferred;
/// 3. otherwise `submission_unknown`, because the member was in a transport's
///    hands and partial effect cannot be excluded.
pub(in crate::relay) fn resolve_from_evidence(
    unit_evidence: Option<SubmissionEvidence>,
    handed_to_transport: bool,
) -> SubmissionEvidence {
    match unit_evidence {
        Some(recorded) => recorded,
        None if handed_to_transport => SubmissionEvidence::SubmissionUnknown,
        None => SubmissionEvidence::NotSubmitted,
    }
}

/// The identities a member carries from authorization to resolution.
///
/// All three are assigned at authorization and never reassigned. `batch` is the
/// unit of authorization, `member` the entry itself, and `attempt` distinguishes
/// one authorization from any other for the same member — which is what lets the
/// relay claim *at most one relay-authorized injection attempt* rather than the
/// stronger at-most-once-delivered claim it cannot support, since transports do
/// not deduplicate attempt ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::relay) struct GuardKey {
    pub(in crate::relay) batch: BatchId,
    pub(in crate::relay) attempt: AttemptId,
    pub(in crate::relay) generation: TransportGeneration,
}

/// The unit of authorization. Authorizing a batch authorizes every member in it
/// atomically; there is no partially-authorized batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::relay) struct BatchId(u64);

/// One authorization of one member, distinct from every other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::relay) struct AttemptId(u64);

/// The transport generation a member was authorized against.
///
/// Carried on the guard key so a replacement generation can be told apart from
/// the one that owned a member: resolving an outcome and proving the old
/// generation stopped executing are different events, and only the second may
/// admit a replacement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::relay) struct TransportGeneration(u64);

impl TransportGeneration {
    pub(in crate::relay) fn value(self) -> u64 {
        self.0
    }
}

static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);

impl BatchId {
    /// Mints the next batch id. Process-local and monotonic; identities never
    /// outlive the relay process, because crash recovery is out of scope and the
    /// ledger is in-memory.
    pub(in crate::relay) fn mint() -> Self {
        Self(NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(in crate::relay) fn value(self) -> u64 {
        self.0
    }
}

impl AttemptId {
    pub(in crate::relay) fn mint() -> Self {
        Self(NEXT_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(in crate::relay) fn value(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The evidence order is relay-private and, until the fence and the watchdog
    /// land, no public path drives a lifecycle trigger — so this is the only
    /// place its three-way discrimination can be exercised. It is worth pinning
    /// directly: the ordering is what keeps a lifecycle event from choosing an
    /// outcome, and each arm asserts something the others cannot.
    #[test]
    fn the_evidence_order_prefers_a_record_then_proof_then_honesty() {
        // A recorded unit result wins outright, even for a member that reached a
        // transport — the record is what the member's siblings resolved from, and
        // disagreeing with it is the split-outcome hazard it exists to prevent.
        assert_eq!(
            resolve_from_evidence(Some(SubmissionEvidence::Submitted), true),
            SubmissionEvidence::Submitted,
        );
        assert_eq!(
            resolve_from_evidence(Some(SubmissionEvidence::NotSubmitted), true),
            SubmissionEvidence::NotSubmitted,
        );

        // No record, never handed over: nothing could have taken effect, so
        // non-delivery is provable rather than inferred.
        assert_eq!(
            resolve_from_evidence(None, false),
            SubmissionEvidence::NotSubmitted,
        );

        // No record, but the member was in a transport's hands: partial effect
        // cannot be excluded, so the honest answer is that we do not know.
        assert_eq!(
            resolve_from_evidence(None, true),
            SubmissionEvidence::SubmissionUnknown,
        );
    }
}
