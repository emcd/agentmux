//! Recording the run an executor is about to submit as one packing unit.

use crate::protocol::identity::ConsumerBinding;
use crate::protocol::mailbox::EntryRange;
use crate::protocol::operations::{DeclareAccepted, DeclareRejection, DeclareResult};

use super::super::super::guard::{GuardKey, PackingUnitId};
use super::super::ledger::{OutstandingDeclaration, UnitRecord, lock_ledger};
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
    });
    for message_id in &member_ids {
        if let Some(entry) = state.entries.get_mut(message_id.as_str()) {
            entry.unit = Some(unit);
            entry.guard = Some(GuardKey::new(entry.sequence));
        }
    }
    state.units.insert(
        unit,
        UnitRecord {
            evidence: None,
            unresolved_members: member_ids.len(),
        },
    );
    Ok(DeclareAccepted { unit, range })
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
    use super::super::ack::ack;
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
    }
}
