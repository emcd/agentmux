//! Resolving a target's mailbox because its transport has been unreachable for
//! longer than the relay allows.
//!
//! This is the one place elapsed duration resolves an entry whose target has not
//! been positively observed torn down, and it is admitted deliberately. What
//! qualifies it is that the duration is measured over an observation made
//! repeatedly rather than standing in for one never made: a target that cannot be
//! reached on every attempt for the whole dwell is evidence, where a target that
//! is merely busy for the same span is not.
//!
//! **The threshold is the relay's and the observation is the transport's.** The
//! executor decides that the condition has held long enough — it is the only
//! layer that can see its own target — and this decides what that means for the
//! entries, which is the split `Transport Health as a Separate Axis` requires.

use crate::protocol::identity::ConsumerBinding;

use super::super::super::guard::SubmissionEvidence;
use super::super::ledger::lock_ledger;
use super::addressing::target_key;
use super::generation::active_generation;
use super::resolution::{ResolvedMember, resolve_positions};

/// Resolves everything a continuously-unreachable target still holds, reporting
/// what each member resolved to.
///
/// Undeclared entries resolve `not_submitted` **directly**, as a plain
/// relay-side transition: such an entry was never bound to a guard and no
/// evidence could still arrive for it, so nothing is being adjudicated and the
/// strong spelling is a fact rather than an inference. A **declared** entry under
/// the same trigger goes through the guard's evidence order instead, because a
/// write may have half-happened for it and only the guard's atomic transition can
/// adjudicate that safely.
///
/// Serialized under the same lock `declare` and `ack` take, so a declaration
/// cannot bind an entry this call is resolving and this call cannot resolve one a
/// declaration has just bound.
///
/// Idempotent by construction: a repeat finds nothing left to resolve. That is
/// what lets the executor drive it from a condition it observes continuously
/// rather than from an edge it would have to detect and remember — and
/// remembering is the part that would go wrong, since an executor that had
/// already fired once would stay silent for entries admitted afterwards.
pub(in crate::relay) fn resolve_unreachable(binding: &ConsumerBinding) -> Vec<ResolvedMember> {
    let Ok(mut state) = lock_ledger() else {
        return Vec::new();
    };
    let key = target_key(&binding.target);
    if active_generation(&state, &key) != Some(binding.generation) {
        // A superseded executor may not resolve a target it no longer holds. Its
        // successor is the one that will observe whether the target is still
        // unreachable, and it starts its own dwell.
        return Vec::new();
    }
    let Some(mailbox) = state.mailboxes.get(&key) else {
        return Vec::new();
    };
    let declared = mailbox.outstanding.map(|outstanding| outstanding.range);
    let positions: Vec<_> = mailbox
        .slots
        .keys()
        .copied()
        .map(|sequence| {
            let is_declared = declared.is_some_and(|range| range.contains(sequence));
            (
                sequence,
                (!is_declared).then_some(SubmissionEvidence::NotSubmitted),
            )
        })
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

/// A sustained unreachability resolves what the target holds, by the route each
/// entry's declaration state calls for.
///
/// Inline for the reason given on the peek block: the ledger is a process-global
/// behind a crate-private lock, and widening these operations to reach them from
/// `tests/` would publish the delivery ledger itself.
///
/// One test because the two routes are one rule seen from both sides. Resolving
/// everything the same way is the failure this guards against, and it is
/// invisible unless both an undeclared and a declared entry are present at the
/// same trigger: the undeclared one must carry the strong spelling that only
/// direct resolution can support, and the declared one must not, because a write
/// may already have reached the target for it.
#[cfg(test)]
mod unreachable_resolution_tests {
    use super::super::declare::declare;
    use super::super::fixtures::{claim, mail, peeked, place, range};
    use super::*;

    #[test]
    fn a_sustained_unreachability_resolves_undeclared_and_declared_entries_differently() {
        let namespace = "mbx-unreachable";
        let bound = claim(namespace);
        for index in 1..=3 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }
        declare(&bound, range(1, 1)).expect("declare the head entry");

        let resolved = resolve_unreachable(&bound);

        let outcomes: Vec<_> = resolved
            .iter()
            .map(|member| (member.message_id.as_str(), member.evidence))
            .collect();
        assert_eq!(
            outcomes,
            vec![
                // Declared: a write was about to begin, and the fence has not
                // established that it did not. `submission_unknown` is the
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
        // starts its own dwell, so letting a stale executor bounce the mailbox
        // would resolve entries the incoming generation is about to serve.
        let stale = super::super::fixtures::binding(namespace, bound.generation.value() + 1);
        assert!(
            resolve_unreachable(&stale).is_empty(),
            "a generation the target does not hold resolves nothing"
        );
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![4],
            "and the entry it would have resolved is still there"
        );
    }
}
