//! Recording the run an executor is about to submit as one packing unit.

use std::time::Duration;

use serde_json::json;

use crate::protocol::identity::ConsumerBinding;
use crate::protocol::mailbox::EntryRange;
use crate::protocol::operations::{DeclareAccepted, DeclareRejection, DeclareResult};
use crate::runtime::inscriptions::emit_inscription;

use super::super::super::guard::{GuardKey, PackingUnitId};
use super::super::ledger::{OutstandingDeclaration, lock_ledger};
use super::addressing::target_key;
use super::generation::active_generation;

/// Records, before any write is attempted, the exact run an executor is about to
/// submit as one packing unit.
///
/// A start record, not a permission grant. It refuses only requests malformed
/// against the mailbox's own state, or ones that would break the
/// one-outstanding-unit invariant; it gates nothing a well-formed executor would
/// ask for, and grants no exclusivity the single-active-generation rule does not
/// already provide.
///
/// The checks run in a fixed order — generation, then outstanding, then position,
/// then extent, then contiguity — so a rejection names one cause rather than
/// whichever check the control flow happened to reach first.
///
/// The outstanding check comes before every check on the range itself, and the
/// order is load-bearing rather than incidental. While a unit is outstanding the
/// cursor has not moved, so *every* range except the outstanding one's own fails
/// the position check too; testing position first would answer a caller that has
/// simply not acknowledged yet with `NotAtCursor`, pointing it at the range it
/// asked for when the range was never the problem. The two rejections also call
/// for different remedies — acknowledge what you hold, versus ask for a different
/// range — so naming the wrong one sends a caller somewhere it cannot make
/// progress.
pub(in crate::relay) fn declare(binding: &ConsumerBinding, range: EntryRange) -> DeclareResult {
    let Ok(mut state) = lock_ledger() else {
        return Err(DeclareRejection::UnknownTarget);
    };
    let target = target_key(&binding.target);
    let Some(mailbox) = state.mailboxes.get(&target) else {
        return Err(DeclareRejection::UnknownTarget);
    };
    if active_generation(&state, &target) != Some(binding.generation) {
        return Err(DeclareRejection::GenerationSuperseded);
    }
    // Before any check on the range asked for, and independent of it. Two
    // declarations of the *same* unacked range would pass every check below
    // unchanged — nothing has acknowledged or resolved those entries, so the
    // cursor still stands where the first declaration found it — and would mint
    // two packing units bound to one entry, which is two guards for one member.
    if let Some(outstanding) = mailbox.outstanding {
        return Err(DeclareRejection::UnitAlreadyOutstanding {
            outstanding: outstanding.unit,
        });
    }
    let expected = mailbox.cursor.next_sequence();
    if range.from() != expected {
        return Err(DeclareRejection::NotAtCursor {
            expected,
            requested: range.from(),
        });
    }
    let highest = mailbox.slots.keys().next_back().copied();
    if highest.is_none_or(|held| range.through() > held) {
        return Err(DeclareRejection::PastMailboxEnd {
            highest,
            requested: range.through(),
        });
    }
    if let Some(absent) = range
        .sequences()
        .find(|sequence| !mailbox.slots.contains_key(sequence))
    {
        return Err(DeclareRejection::NotContiguous { absent });
    }
    let unit = PackingUnitId::mint();
    let member_ids: Vec<String> = range
        .sequences()
        .filter_map(|sequence| {
            mailbox
                .slots
                .get(&sequence)
                .map(|slot| slot.message_id.clone())
        })
        .collect();
    let mailbox = state.mailboxes.get_mut(&target).expect("mailbox present");
    mailbox.outstanding = Some(OutstandingDeclaration {
        unit,
        range,
        generation: binding.generation,
        declared_at: std::time::Instant::now(),
    });
    for message_id in &member_ids {
        if let Some(entry) = state.entries.get_mut(message_id.as_str()) {
            entry.unit = Some(unit);
            entry.guard = Some(GuardKey::new(entry.sequence));
        }
    }
    // The partition is otherwise invisible. Every other step of a delivery leaves
    // a record, but which members shared a write — and so which of them a single
    // target-side failure could have taken down together — could not be answered
    // from the log at all. That is a gap in an arc whose subject is per-member
    // attribution: a reader could see two members resolve identically without
    // being able to tell whether they shared a unit or merely agreed.
    //
    // Emitted with the ledger lock still held, which is deliberate here where
    // resolutions are deliberately handed back to be reported after release. A
    // resolution reaches another target's worker and locks of its own; this
    // writes one line about state that has just been committed, and moving it
    // outside would let a reader see the acknowledgment of a unit whose
    // declaration had not been recorded yet.
    emit_inscription(
        "relay.delivery.partition.declared",
        &json!({
            "unit_id": unit.value(),
            "member_ids": member_ids,
            "member_count": member_ids.len(),
            "from_sequence": range.from().value(),
            "through_sequence": range.through().value(),
        }),
    );
    Ok(DeclareAccepted { unit, range })
}

/// How long the target's outstanding declaration has been outstanding, or `None`
/// when it has none.
///
/// The execution watchdog's read. It is a question about the *relay's* record
/// rather than about the transport, which is what lets the bound be described as
/// one over the relay's own supervised execution: an unacknowledged declaration
/// past the bound says the executor overran, and says nothing at all about the
/// target's health.
///
/// Scoped to the caller's generation. A worker supervising its own generation
/// must not arm its watchdog on a declaration a successor made, and after a
/// replacement the outgoing declaration is resolved rather than inherited.
pub(in crate::relay) fn declaration_age(binding: &ConsumerBinding) -> Option<Duration> {
    let state = lock_ledger().ok()?;
    let key = target_key(&binding.target);
    if active_generation(&state, &key) != Some(binding.generation) {
        return None;
    }
    let outstanding = state.mailboxes.get(&key)?.outstanding?;
    (outstanding.generation == binding.generation).then(|| outstanding.declared_at.elapsed())
}

/// A declaration binds one well-formed range, and only one at a time.
///
/// Inline for the reason given on the peek block above. One test because the
/// refusals are not independent: what makes the binding safe is that *every* way
/// a range can be wrong is refused before a `PackingUnitId` is minted, and a
/// suite that checked them separately could pass while a sixth shape slipped
/// through the ordering between them.
#[cfg(test)]
mod mailbox_declaration_tests {
    use super::super::fixtures::{
        acknowledge, admit_only, binding, claim, mail, place, range, seq,
    };
    use crate::protocol::operations::MemberAcknowledgment;

    use super::super::super::super::guard::SubmissionEvidence;
    use super::*;

    #[test]
    fn a_declaration_binds_one_well_formed_range_at_a_time() {
        let namespace = "mbx-declare";
        let bound = claim(namespace);
        for index in 1..=5 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }

        assert_eq!(
            declare(
                &binding(namespace, bound.generation.value() + 1),
                range(1, 1)
            ),
            Err(DeclareRejection::GenerationSuperseded),
            "a generation the target does not hold binds nothing"
        );
        assert_eq!(
            declare(&bound, range(2, 3)),
            Err(DeclareRejection::NotAtCursor {
                expected: seq(1),
                requested: seq(2),
            }),
            "a range that does not begin at the cursor plus one is refused"
        );
        assert_eq!(
            declare(&bound, range(1, 9)),
            Err(DeclareRejection::PastMailboxEnd {
                highest: Some(seq(5)),
                requested: seq(9),
            }),
            "a range past what the mailbox holds is refused"
        );

        let accepted = declare(&bound, range(1, 3)).expect("a well-formed range binds");
        assert_eq!(accepted.range, range(1, 3));

        // The teeth of the single-outstanding rule. The identical range passes
        // every position and contiguity check unchanged — nothing has
        // acknowledged or resolved those entries, so the cursor still stands
        // where the first declaration found it — so without this the relay would
        // mint a second unit over entries already bound to the first.
        assert_eq!(
            declare(&bound, range(1, 3)),
            Err(DeclareRejection::UnitAlreadyOutstanding {
                outstanding: accepted.unit,
            }),
            "a second declaration of the outstanding range mints nothing"
        );
        // And a *non-overlapping* range is refused too, which is what separates
        // this from an overlap check: declaration is totally ordered per target,
        // not merely non-overlapping.
        assert_eq!(
            declare(&bound, range(4, 5)),
            Err(DeclareRejection::UnitAlreadyOutstanding {
                outstanding: accepted.unit,
            }),
            "an outstanding unit blocks even a range that shares no entry with it"
        );

        acknowledge(
            &bound,
            accepted.unit,
            &[
                MemberAcknowledgment {
                    sequence: seq(1),
                    evidence: SubmissionEvidence::Submitted,
                },
                MemberAcknowledgment {
                    sequence: seq(2),
                    evidence: SubmissionEvidence::Submitted,
                },
                MemberAcknowledgment {
                    sequence: seq(3),
                    evidence: SubmissionEvidence::Submitted,
                },
            ],
        )
        .expect("the outstanding unit resolves");
        assert!(
            declare(&bound, range(4, 5)).is_ok(),
            "declaring again is possible once the outstanding unit is acknowledged"
        );

        // A hole in the numbering is refused by name rather than silently
        // shortening the bound range, because a transport that asked for five
        // entries and was handed three would write a set the relay did not record.
        let gapped = "mbx-declare-gap";
        let gapped_bound = claim(gapped);
        place(gapped, "mbx-declare-gap-1", 1, mail("body"));
        admit_only(gapped, "mbx-declare-gap-2", 1);
        place(gapped, "mbx-declare-gap-3", 1, mail("body"));
        assert_eq!(
            declare(&gapped_bound, range(1, 3)),
            Err(DeclareRejection::NotContiguous { absent: seq(2) }),
            "a range spanning a position the mailbox does not hold is refused"
        );
        // And bound nothing on its way out, which is what makes every refusal
        // above recoverable rather than terminal for the target. A rejection
        // that had already recorded an outstanding unit would answer this with
        // `UnitAlreadyOutstanding`, leaving the mailbox holding a unit no
        // executor ever declared and none can acknowledge.
        assert!(
            declare(&gapped_bound, range(1, 1)).is_ok(),
            "a refused declaration binds nothing, so the head is still declarable"
        );
    }
}

/// A declaration and a replacement of the generation that made it cannot both
/// take effect.
///
/// Its own block for the reason the one above has its own. One test because
/// there is one property, seen from whichever side reached the lock first: a
/// declaration is a state mutation, so it needs the same serialization against a
/// generation flip that an acknowledgment does, and a generation check alone
/// would not give it one.
///
/// **The interleaving is established, not arranged.** Releasing two threads
/// together proves nothing about either reaching the lock, and measuring which
/// one won proves nothing either — an even split is exactly what running one
/// thread to completion in random order produces. So this uses the same lock
/// boundary the acknowledgment's contention block does, and for the same reason;
/// the full argument is there. Here the declaration is held inside the critical
/// section while the replacement is watched reaching the boundary and failing to
/// enter, and the second phase runs the flip first so the declaration behind it
/// is late by construction.
#[cfg(test)]
mod declaration_serialization_tests {
    use crate::protocol::identity::ConsumerBinding;

    use super::super::super::super::fence::FenceVerdict;
    use super::super::super::super::guard::SubmissionEvidence;
    use super::super::super::lock_boundary;
    use super::super::fixtures::{claim, mail, peeked, place, range, reserved_envelopes, target};
    use super::super::generation::replace_consumer_generation;
    use super::*;

    /// A claimed target holding three undeclared entries.
    fn three_queued(namespace: &str) -> ConsumerBinding {
        let outgoing = claim(namespace);
        for index in 1..=3 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }
        outgoing
    }

    #[test]
    fn a_declaration_and_a_replacement_never_both_take_effect() {
        let namespace = "mbx-declare-inflight";
        let outgoing = three_queued(namespace);

        let boundary = lock_boundary::watch();
        let declaring = {
            let binding = outgoing.clone();
            boundary.spawn(move || declare(&binding, range(1, 3)))
        };
        // Inside the critical section with the guard in hand: a declaration
        // genuinely in flight, not merely dispatched.
        boundary.await_holder();

        let replacing = {
            let target = target(namespace);
            let generation = outgoing.generation;
            boundary.spawn(move || {
                replace_consumer_generation(&target, generation, FenceVerdict::Positive)
                    .expect("a fenced incumbent is replaced")
            })
        };
        // The replacement has reached the boundary and cannot cross it. A flip
        // that could overtake a declaration already inside would leave a packing
        // unit owned by a generation that no longer holds the target.
        boundary.await_arrival();
        boundary.assert_none_entered();

        boundary.release();
        let declared = declaring.join().expect("the declaring thread");
        let replacement = replacing.join().expect("the replacing thread");
        let incoming = ConsumerBinding::new(target(namespace), replacement.generation);

        assert_eq!(
            declared
                .expect("the declaration that was inside the lock binds")
                .range,
            range(1, 3),
            "a declaration inside the critical section binds the whole run it named"
        );
        // And the replacement behind it inherits that declaration rather than an
        // empty mailbox. It has to resolve it: the record says a write was about
        // to begin, and re-serving those entries to the incoming generation
        // could write them a second time.
        assert_eq!(
            replacement
                .resolved
                .iter()
                .map(|member| (member.evidence, member.declared))
                .collect::<Vec<_>>(),
            vec![(SubmissionEvidence::SubmissionUnknown, true); 3],
            "and the replacement resolves what it bound, through the guard"
        );
        assert_eq!(
            peeked(&incoming, 10, 1_000),
            Vec::<u64>::new(),
            "so the incoming generation is handed nothing that was about to be written"
        );
        assert_eq!(
            reserved_envelopes(namespace),
            0,
            "and every resolved member released its reservation"
        );

        // The other direction, late by construction rather than by scheduling:
        // the flip has already happened when the declaration arrives. It must
        // bind nothing, and the entries it named must survive undisturbed for
        // the generation that now owns them.
        let namespace = "mbx-declare-late";
        let outgoing = three_queued(namespace);
        let replacement = replace_consumer_generation(
            &target(namespace),
            outgoing.generation,
            FenceVerdict::Positive,
        )
        .expect("a fenced incumbent is replaced");
        let incoming = ConsumerBinding::new(target(namespace), replacement.generation);

        assert_eq!(
            declare(&outgoing, range(1, 3)),
            Err(DeclareRejection::GenerationSuperseded),
            "a declaration reaching the relay after the flip is refused on its generation"
        );
        assert!(
            replacement.resolved.is_empty(),
            "the replacement had no declaration to resolve, so it resolved nothing"
        );
        assert_eq!(
            peeked(&incoming, 10, 1_000),
            vec![1, 2, 3],
            "the entries the refused declaration named stay queued for the current generation"
        );
        assert!(
            declare(&incoming, range(1, 3)).is_ok(),
            "and undeclared, so the incoming generation may bind them itself"
        );
        assert_eq!(
            reserved_envelopes(namespace),
            3,
            "the refusal resolved nothing, so it released no reservation"
        );
    }
}
