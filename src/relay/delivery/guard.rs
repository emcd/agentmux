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
// Both are transport-facing: `PartitionSink`'s signature names them, and
// `src/transports` may not import `crate::relay`, so they are defined there and
// re-exported here. The guard still owns what they *mean* — the evidence order
// below and the outcome mapping on `SubmissionEvidence` are relay-private.
pub(in crate::relay) use crate::transports::{PackingUnitId, SubmissionEvidence};

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
    /// This member's bundle is being torn down while the relay keeps serving
    /// others. States that the owning worker ended, not that the target failed:
    /// the bundle stopped, which says nothing about whether the target was well.
    BundleStop,
    /// The relay's own supervised execution ran past
    /// `[delivery].submission-timeout-ms` and its generation fence reached a
    /// verdict with this member still unresolved.
    ///
    /// Like every other variant, it states when the owning path gave up, not what
    /// happened at the target: the bound is over relay-side execution and asserts
    /// nothing about target health.
    ExecutionBound,
}

impl GuardTrigger {
    /// The reason string recorded alongside the resolved outcome, naming the
    /// trigger rather than restating the evidence.
    pub(in crate::relay) fn reason(self) -> &'static str {
        match self {
            Self::CollectorPanic => "delivery collector task panicked before resolving",
            Self::ChannelClosed => "transport outcome channel closed before resolving",
            Self::GracefulShutdown => "relay shut down before this member resolved",
            Self::BundleStop => "bundle stopped before this member resolved",
            Self::ExecutionBound => {
                "relay execution exceeded the submission bound before this member resolved"
            }
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
/// 2. otherwise `not_submitted`, when the member was never bound to a packing
///    unit — the partition is recorded before the first target-side effect, so
///    an unbound member provably could not have been submitted;
/// 3. otherwise `submission_unknown`, because the member's unit was in a
///    transport's hands and partial effect cannot be excluded.
///
/// **The discriminator is unit binding, not the manner of failure.** Refusal,
/// panic and cancellation all arrive here identically; what separates a provable
/// `not_submitted` from an honest `submission_unknown` is whether a unit had
/// been recorded for the member when the trigger fired.
pub(in crate::relay) fn resolve_from_evidence(
    unit_evidence: Option<SubmissionEvidence>,
    unit: Option<PackingUnitId>,
) -> SubmissionEvidence {
    match unit_evidence {
        Some(recorded) => recorded,
        None if unit.is_some() => SubmissionEvidence::SubmissionUnknown,
        None => SubmissionEvidence::NotSubmitted,
    }
}

/// The identities a member carries from authorization to resolution.
///
/// Both are assigned at authorization and never reassigned. `batch` is the unit
/// of authorization, and `attempt` distinguishes one authorization from any
/// other for the same member — which is what lets the relay claim *at most one
/// relay-authorized injection attempt* rather than the stronger
/// at-most-once-delivered claim it cannot support, since transports do not
/// deduplicate attempt ids.
///
/// There is deliberately no generation component. A generation is the worker
/// that owns a transport for its lifetime, and a fence verdict ends both
/// together — so a member cannot outlive the generation it was authorized
/// against, and a key naming one would only ever carry a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::relay) struct GuardKey {
    pub(in crate::relay) batch: BatchId,
    pub(in crate::relay) attempt: AttemptId,
}

/// The unit of authorization. Authorizing a batch authorizes every member in it
/// atomically; there is no partially-authorized batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::relay) struct BatchId(u64);

/// One authorization of one member, distinct from every other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::relay) struct AttemptId(u64);

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

    /// The evidence order is relay-private, and the lifecycle triggers that drive
    /// it — shutdown, a collector panic, the execution bound — each reach only one
    /// or two of its arms from any single public path. Pinning it directly is what
    /// exercises the three-way discrimination as a whole: the ordering is what
    /// keeps a lifecycle event from choosing an outcome, and each arm asserts
    /// something the others cannot.
    #[test]
    fn the_evidence_order_prefers_a_record_then_proof_then_honesty() {
        // A recorded unit result wins outright, even for a member that reached a
        // transport — the record is what the member's siblings resolved from, and
        // disagreeing with it is the split-outcome hazard it exists to prevent.
        assert_eq!(
            resolve_from_evidence(
                Some(SubmissionEvidence::Submitted),
                Some(PackingUnitId::mint())
            ),
            SubmissionEvidence::Submitted,
        );
        assert_eq!(
            resolve_from_evidence(
                Some(SubmissionEvidence::NotSubmitted),
                Some(PackingUnitId::mint())
            ),
            SubmissionEvidence::NotSubmitted,
        );

        // No record, never bound to a unit: the partition is recorded before the
        // first target-side effect, so non-delivery is provable rather than
        // inferred.
        assert_eq!(
            resolve_from_evidence(None, None),
            SubmissionEvidence::NotSubmitted,
        );

        // No record, but the member's unit was in a transport's hands: partial
        // effect cannot be excluded, so the honest answer is that we do not know.
        assert_eq!(
            resolve_from_evidence(None, Some(PackingUnitId::mint())),
            SubmissionEvidence::SubmissionUnknown,
        );
    }
}
