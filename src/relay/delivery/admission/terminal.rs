//! The single terminal transition, which is also the only place admission quota
//! is released.
//!
//! The state check, the entry removal, both quota releases and the unit-record
//! decrement all run under one acquisition of the ledger lock, because the
//! terminal transition and the quota release are one atomic operation. Duplicate
//! completions converge here rather than racing: exactly one caller can observe
//! [`TerminalTransition::Won`].

use super::super::guard::{GuardKey, QueueEntryState, SubmissionEvidence, resolve_from_evidence};
use super::ledger::lock_ledger;

/// The outcome of attempting the single terminal transition for one member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum TerminalTransition {
    /// This caller performed the transition; admission quota was released here
    /// and nowhere else. `evidence` is what the guard's evidence order resolves
    /// the member to when the caller has no outcome of its own, and `guard`
    /// carries the identities the member was authorized under — `None` for a
    /// member terminalized while still `Pending`, which was never authorized and
    /// so has none.
    Won {
        evidence: SubmissionEvidence,
        /// Whether the member was bound to a packing unit.
        ///
        /// Reported separately because `evidence` cannot carry it: an **unbound**
        /// member resolves `NotSubmitted`, and a **bound** member whose unit
        /// recorded `NotSubmitted` resolves `NotSubmitted` too. Same value,
        /// different authority — the first is the guard's inference from absence,
        /// the second is a transport's report of what its write proved.
        ///
        /// Only the second may override a producer's own outcome. Treating the
        /// first as authoritative would let a transport that wrote before
        /// declaring have its `delivered` replaced by a provable-non-delivery
        /// claim, which is the one direction this contract must never invent.
        bound: bool,
        guard: Option<GuardKey>,
    },
    /// Another caller already terminalized this member. Report nothing: doing so
    /// would be the duplicate resolution the guard exists to prevent.
    AlreadyTerminal,
    /// No reservation is held under this id.
    ///
    /// Deliberately **not** a licence to report. Two very different situations
    /// produce it: a relay-originated receipt, which bypassed admission because
    /// nothing was ever accepted for it, and an admitted member whose terminal
    /// transition another caller already won and cleaned up. The winning
    /// transition removes the entry — it has to, or the ledger would grow by one
    /// record per message the relay ever delivered — so absence cannot
    /// distinguish them on its own.
    ///
    /// Callers MUST resolve the ambiguity from the task, reporting only for work
    /// known to have bypassed admission. Treating absence as reportable is what
    /// let two competing resolvers each emit an outcome for one accepted member.
    NoReservation,
}

/// Attempts the single terminal transition for one member, releasing its
/// admission quota if and only if this caller wins.
///
/// Duplicate completions converge here rather than racing: the state check and
/// the release happen under one lock, so exactly one caller can observe
/// [`TerminalTransition::Won`]. Every lifecycle trigger routes through this same
/// call, and none of them chooses the outcome — the returned `evidence` comes
/// from the guard's one evidence order.
pub(in crate::relay) fn terminalize(message_id: &str) -> TerminalTransition {
    let Ok(mut state) = lock_ledger() else {
        // A poisoned ledger cannot be reasoned about, and reporting an outcome
        // whose uniqueness we cannot establish is worse than reporting none.
        return TerminalTransition::AlreadyTerminal;
    };
    let Some(entry) = state.entries.get(message_id).cloned() else {
        return TerminalTransition::NoReservation;
    };
    if entry.state == QueueEntryState::Terminal {
        return TerminalTransition::AlreadyTerminal;
    }
    state.entries.remove(message_id);
    state.global.envelopes = state.global.envelopes.saturating_sub(1);
    state.global.bytes = state.global.bytes.saturating_sub(entry.canonical_bytes);
    if let Some(usage) = state.per_target.get_mut(&entry.target) {
        usage.envelopes = usage.envelopes.saturating_sub(1);
        usage.bytes = usage.bytes.saturating_sub(entry.canonical_bytes);
        if usage.envelopes == 0 && usage.bytes == 0 {
            state.per_target.remove(&entry.target);
        }
    }
    // The unit record is read before it is released, so the last member of a unit
    // still resolves from the same evidence its groupmates did.
    let unit_evidence = entry.unit.and_then(|unit| {
        let record = state.units.get_mut(&unit)?;
        let evidence = record.evidence;
        record.unresolved_members = record.unresolved_members.saturating_sub(1);
        if record.unresolved_members == 0 {
            state.units.remove(&unit);
        }
        evidence
    });
    TerminalTransition::Won {
        evidence: resolve_from_evidence(unit_evidence, entry.unit),
        bound: entry.unit.is_some(),
        guard: entry.guard,
    }
}

/// The write-once terminal transition, contested.
///
/// Its own block for the reason the two above have theirs: each already carries a
/// test. Inline because the ledger is `pub(in crate::relay)` by design — the
/// public seam for transports is `PartitionSink`, and widening `terminalize` to
/// reach it from `tests/` would publish the delivery ledger itself.
///
/// What makes this worth a test rather than a comment is that nothing else
/// exercises the gate under contention. A probe on `terminalize` across every
/// exactly-once test in this arc — the flapping target, the replaced generation,
/// the mixed shutdown — records exactly **one** attempt per message. Their
/// uniqueness assertions are therefore tripwires against a future second
/// resolver, not demonstrations that the transition adjudicates one. Two
/// resolvers do race in production, at the seam between a collector resolving a
/// member and a fence cutting the same member short, and that race is not
/// constructible from outside the relay.
#[cfg(test)]
mod terminal_contention_tests {
    use super::*;
    use crate::configuration::SessionType;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::super::admit::admit;
    use super::super::ledger::AdmissionTargetKey;

    /// Exactly one of many simultaneous resolvers may report a member.
    ///
    /// The barrier is what makes this a race rather than a sequence: without it
    /// the threads would start far enough apart that the first would finish
    /// before the second began, and the test would re-check the same
    /// already-terminal path the arc's integration tests already take.
    ///
    /// Teeth: leaving the entry in the ledger after a win — the removal is the
    /// actual gate, not the state comparison beside it — makes all eight win.
    #[test]
    fn only_one_of_many_racing_resolvers_wins_a_member() {
        const RESOLVERS: usize = 8;
        const MESSAGE: &str = "contention-test-member";

        let target = AdmissionTargetKey::new(
            "contention-test",
            Path::new("/nonexistent/contention-test"),
            "target",
        );
        admit(MESSAGE, target, SessionType::Tmux, 1).expect("admit");

        let winners = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(RESOLVERS));
        let handles: Vec<_> = (0..RESOLVERS)
            .map(|_| {
                let winners = Arc::clone(&winners);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    if matches!(terminalize(MESSAGE), TerminalTransition::Won { .. }) {
                        winners.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("resolver thread");
        }

        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "a member owes exactly one answer no matter how many resolvers reach it"
        );
    }
}
