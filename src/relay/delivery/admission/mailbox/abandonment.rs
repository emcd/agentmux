//! Resolving everything a target's mailbox holds, because nothing will serve it.
//!
//! Three findings reach here, and they are the three ways a target stops having
//! a future rather than merely being slow:
//!
//! - its transport has been **continuously unreachable** past the relay's dwell,
//!   which its own executor observes and reports;
//! - its generation was **fail-stopped** by a negative fence verdict, so no
//!   replacement may be elected and nothing will ever peek this mailbox again;
//! - the relay is **shutting down**.
//!
//! All three are the same act on the ledger and differ only in what a sender is
//! told, so the transition lives here once and the caller supplies the words.
//! The dwell is the only one that admits elapsed duration as evidence, and it is
//! admitted deliberately: the duration qualifies an observation made repeatedly
//! rather than standing in for one never made. A target that cannot be reached
//! on every attempt for the whole dwell is evidence; a target that is merely
//! busy for the same span is not.
//!
//! **The threshold is the relay's and the observation is the transport's.** The
//! executor decides that the condition has held long enough — it is the only
//! layer that can see its own target — and this decides what that means for the
//! entries, which is the split `Transport Health as a Separate Axis` requires.

use crate::protocol::identity::ConsumerBinding;

use super::super::ledger::lock_ledger;
use super::addressing::target_key;
use super::generation::active_generation;
use super::resolution::{ResolvedMember, resolve_positions};

/// Resolves everything the target still holds, reporting what each member
/// resolved to.
///
/// **What separates an undeclared entry from a declared one is the guard's own
/// evidence order, not a value this function chooses.** An undeclared entry is
/// bound to no packing unit, so the order reads `not_submitted` for it — a
/// positive claim, and a sound one, because nothing was ever about to write it
/// and no evidence could still arrive. A declared entry is bound, so the same
/// order reads `submission_unknown`: a write was about to begin and the trigger
/// establishes only that it will not finish, never that it did not start.
/// Supplying either value here would state as a constant what the boundness
/// discriminator already decides, and would then be wrong for the other kind.
///
/// Serialized under the same lock `declare` and `ack` take, so a declaration
/// cannot bind an entry this call is resolving and this call cannot resolve one
/// a declaration has just bound.
///
/// Idempotent by construction: a repeat finds nothing left to resolve. That is
/// what lets the dwell drive it from a condition observed continuously rather
/// than from an edge the executor would have to detect and remember — and
/// remembering is the part that would go wrong, since an executor that had
/// already fired once would stay silent for entries admitted afterwards.
pub(in crate::relay) fn resolve_target_entries(binding: &ConsumerBinding) -> Vec<ResolvedMember> {
    let Ok(mut state) = lock_ledger() else {
        return Vec::new();
    };
    let key = target_key(&binding.target);
    if active_generation(&state, &key) != Some(binding.generation) {
        // A superseded caller may not resolve a target it no longer holds. Its
        // successor is the one that will observe whether the condition persists,
        // and it starts its own dwell.
        return Vec::new();
    }
    let Some(mailbox) = state.mailboxes.get(&key) else {
        return Vec::new();
    };
    let positions: Vec<_> = mailbox
        .slots
        .keys()
        .copied()
        .map(|sequence| (sequence, None))
        .collect();
    let resolved = resolve_positions(&mut state, &key, positions);
    // The declaration named entries that no longer exist, so leaving it in place
    // would refuse every later declaration for this target with a unit nobody can
    // acknowledge. Recorded as resolved rather than forgotten, so an executor
    // that acknowledges it afterwards is told what happened.
    if let Some(mailbox) = state.mailboxes.get_mut(&key) {
        mailbox.reconcile_outstanding();
    }
    resolved
}

/// Abandoning a target resolves what it holds, by the route each entry's
/// declaration state calls for.
///
/// Inline for the reason given on the peek block: the ledger is a process-global
/// behind a crate-private lock, and widening these operations to reach them from
/// `tests/` would publish the delivery ledger itself.
///
/// One test because the two routes are one rule seen from both sides. Resolving
/// everything the same way is the failure this guards against, and it is
/// invisible unless both an undeclared and a declared entry are present at the
/// same trigger: the undeclared one must carry the strong spelling that only its
/// unboundness can support, and the declared one must not, because a write may
/// already have reached the target for it.
#[cfg(test)]
mod abandonment_tests {
    use super::super::super::super::guard::SubmissionEvidence;
    use super::super::declare::declare;
    use super::super::fixtures::{binding, claim, mail, peeked, place, range};
    use super::*;

    #[test]
    fn abandoning_a_target_resolves_undeclared_and_declared_entries_differently() {
        let namespace = "mbx-abandon";
        let bound = claim(namespace);
        for index in 1..=3 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }
        declare(&bound, range(1, 1)).expect("declare the head entry");

        let resolved = resolve_target_entries(&bound);

        let outcomes: Vec<_> = resolved
            .iter()
            .map(|member| (member.message_id.as_str(), member.evidence))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                // Declared: a write was about to begin, and nothing here
                // establishes that it did not. `submission_unknown` is the
                // guard's reading of that, and the honest one.
                (
                    format!("{namespace}-1").as_str(),
                    SubmissionEvidence::SubmissionUnknown
                ),
                // Undeclared: nothing was ever about to write these, so nothing
                // reached the target and the relay can say so.
                (
                    format!("{namespace}-2").as_str(),
                    SubmissionEvidence::NotSubmitted
                ),
                (
                    format!("{namespace}-3").as_str(),
                    SubmissionEvidence::NotSubmitted
                ),
            ],
            "each entry resolves by the route its declaration state calls for"
        );
        assert!(
            resolved.first().is_some_and(|member| member.declared),
            "the declared member is reported as one, so a caller can spell its outcome apart"
        );
        assert!(
            resolved.iter().skip(1).all(|member| !member.declared),
            "and the undeclared ones are reported as undeclared"
        );
        assert_eq!(
            peeked(&bound, 10, 1_000),
            Vec::<u64>::new(),
            "the target holds nothing further to serve"
        );
        // The abandoned declaration must not outlive the entries it named, or
        // every later declaration for this target is refused with a unit nobody
        // can acknowledge.
        place(namespace, &format!("{namespace}-4"), 1, mail("body"));
        assert!(
            declare(&bound, range(4, 4)).is_ok(),
            "a declaration whose members were all resolved here stops blocking"
        );

        // A superseded caller resolves nothing. Its successor owns the target and
        // makes its own findings, so letting a stale executor bounce the mailbox
        // would resolve entries the incoming generation is about to serve.
        let stale = binding(namespace, bound.generation.value() + 1);
        assert!(
            resolve_target_entries(&stale).is_empty(),
            "a generation the target does not hold resolves nothing"
        );
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![4],
            "and the entry it would have resolved is still there"
        );
    }
}
