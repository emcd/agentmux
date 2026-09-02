//! Terminalizing the entries a declaration bound, from what the executor
//! observed writing them.

use crate::protocol::identity::ConsumerBinding;
use crate::protocol::mailbox::EntrySequence;
use crate::protocol::operations::{AckAccepted, AckRejection, AckResult, MemberAcknowledgment};

use super::super::super::guard::PackingUnitId;
use super::super::ledger::lock_ledger;
use super::addressing::target_key;
use super::generation::active_generation;
use super::resolution::{ResolvedMember, resolve_positions};

/// What an acknowledgment did, and what its caller must now report.
///
/// The two travel together because they are produced under one acquisition of
/// the ledger lock and consumed on opposite sides of its release: the result is
/// the executor's answer, and the resolved members are the senders' — each owed
/// a terminal outcome, and for a non-delivered one a receipt, neither of which
/// may be emitted from inside the lock.
///
/// Handed back rather than published here for the same reason a replacement
/// hands back what it resolved: the ledger resolves and the caller reports, so
/// swallowing these would turn a loud outcome into a silent one for exactly the
/// members whose writes were most likely to have gone wrong.
#[derive(Debug)]
pub(in crate::relay) struct Acknowledgment {
    pub(in crate::relay) result: AckResult,
    pub(in crate::relay) resolved: Vec<ResolvedMember>,
}

/// Terminalizes exactly the entries a prior declaration bound, from what the
/// executor observed writing them.
///
/// The acknowledged range comes from the relay's own declaration record rather
/// than from the caller. A caller holding a valid generation binding but no
/// declaration can therefore advance no cursor and release no quota — the gap a
/// caller-supplied endpoint would leave open.
pub(in crate::relay) fn ack(
    binding: &ConsumerBinding,
    unit: PackingUnitId,
    members: &[MemberAcknowledgment],
) -> Acknowledgment {
    let mut resolved = Vec::new();
    let result = apply(binding, unit, members, &mut resolved);
    Acknowledgment { result, resolved }
}

/// The acknowledgment itself, with everything it resolves collected into
/// `resolved` for the caller to report once the lock is gone.
fn apply(
    binding: &ConsumerBinding,
    unit: PackingUnitId,
    members: &[MemberAcknowledgment],
    resolved: &mut Vec<ResolvedMember>,
) -> AckResult {
    let Ok(mut state) = lock_ledger() else {
        return Err(AckRejection::UnknownTarget);
    };
    let target = target_key(&binding.target);
    let Some(mailbox) = state.mailboxes.get(&target) else {
        return Err(AckRejection::UnknownTarget);
    };
    if active_generation(&state, &target) != Some(binding.generation) {
        return Err(AckRejection::GenerationSuperseded);
    }
    let outstanding = match mailbox.outstanding {
        Some(outstanding) if outstanding.unit == unit => outstanding,
        // Not the outstanding unit. A unit this target acknowledged already is a
        // no-op — it names a real binding that has resolved — while anything else
        // names one that never existed for this caller to resolve.
        _ => {
            return match mailbox.acknowledged.get(&unit) {
                Some(range) => Ok(AckAccepted::AlreadyTerminalized { range: *range }),
                None => Err(AckRejection::UnitNotDeclared),
            };
        }
    };
    // A declaration made under an earlier generation is not this caller's to
    // resolve, even though the identifier is real: the consumer that bound it no
    // longer owns the target.
    if outstanding.generation != binding.generation {
        return Err(AckRejection::UnitNotDeclared);
    }

    let range = outstanding.range;
    // Validated in full before anything mutates, because a partially applied
    // acknowledgment cannot be taken back: the members it already terminalized
    // are resolved for good.
    //
    // The match must be exact — every position once, and nothing else. A missing
    // member has no report to be resolved from, and the only ways to proceed
    // would be to invent one or to borrow a sibling's, both of which state an
    // outcome nothing observed for that member. A repeated or out-of-range member
    // means the caller is describing a unit other than the one it declared.
    let covered: Vec<EntrySequence> = members.iter().map(|member| member.sequence).collect();
    let evidence_matches_unit = covered.len() as u64 == range.entries_count()
        && range
            .sequences()
            .all(|sequence| covered.iter().filter(|held| **held == sequence).count() == 1);
    if !evidence_matches_unit {
        return Err(AckRejection::EvidenceDoesNotCoverUnit { expected: range });
    }

    // The unit's shared record still carries a value, because a sibling resolved
    // by a concurrent lifecycle trigger reads it rather than the caller's list.
    // Every member below resolves from its own report regardless, so this decides
    // nothing that the evidence supplied here answers for.
    if let Some(first) = members.first()
        && let Some(record) = state.units.get_mut(&unit)
        && record.evidence.is_none()
    {
        record.evidence = Some(first.evidence);
    }

    // Each member resolves from its own report rather than from the unit's
    // shared record: a write can submit some members of a unit and fail on
    // others, and the record cannot express that.
    resolved.extend(resolve_positions(
        &mut state,
        &target,
        members
            .iter()
            .map(|member| (member.sequence, Some(member.evidence))),
    ));

    // Each member's position was retired as it terminalized, which already
    // carried the cursor over the range. It is read back rather than assigned so
    // that a position retired ahead of the range — a rollback, or an entry a
    // lifecycle trigger resolved — is not undone by moving the cursor backwards
    // onto it.
    let mailbox = state.mailboxes.get_mut(&target).expect("mailbox present");
    for sequence in range.sequences() {
        mailbox.retire(sequence);
    }
    mailbox.outstanding = None;
    mailbox.acknowledged.insert(unit, range);
    Ok(AckAccepted::Terminalized {
        range,
        cursor: mailbox.cursor,
    })
}

/// An acknowledgment reports one outcome per member, and each member resolves
/// from its own.
///
/// Inline for the reason given on the peek block above. One test because the
/// validation and the per-member plumbing are one property: the validation is
/// what makes it impossible for a member to arrive without a report, and the
/// plumbing is what keeps a member that has one from being resolved by a
/// sibling's instead.
#[cfg(test)]
mod mailbox_evidence_tests {
    use super::super::super::super::guard::SubmissionEvidence;
    use super::super::super::authorize::record_unit_evidence;
    use super::super::super::ledger::lock_ledger;
    use super::super::super::terminal::{TerminalTransition, release_entry};
    use super::super::declare::declare;
    use super::super::fixtures::{acknowledge, claim, mail, peeked, place, range, seq};
    use super::*;

    #[test]
    fn an_acknowledgment_needs_one_report_per_member_and_uses_each() {
        let namespace = "mbx-evidence";
        let bound = claim(namespace);
        for index in 1..=3 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }
        let accepted = declare(&bound, range(1, 3)).expect("declare");

        let report = |sequence: u64, evidence: SubmissionEvidence| MemberAcknowledgment {
            sequence: seq(sequence),
            evidence,
        };
        let rejected = Err(AckRejection::EvidenceDoesNotCoverUnit {
            expected: range(1, 3),
        });

        // Short by one. This is the shape with teeth: resolving the omitted
        // member from a sibling's report would terminalize it with an outcome
        // nothing observed for it, and `Submitted` is chosen for the reports that
        // are present precisely because leaking it onto member 3 would claim a
        // write that may never have happened.
        assert_eq!(
            acknowledge(
                &bound,
                accepted.unit,
                &[
                    report(1, SubmissionEvidence::Submitted),
                    report(2, SubmissionEvidence::Submitted),
                ],
            ),
            rejected,
            "an acknowledgment missing a member is refused rather than filled in"
        );
        assert_eq!(
            acknowledge(
                &bound,
                accepted.unit,
                &[
                    report(1, SubmissionEvidence::Submitted),
                    report(1, SubmissionEvidence::Submitted),
                    report(2, SubmissionEvidence::Submitted),
                ],
            ),
            rejected,
            "a repeated member is refused, since it leaves another unreported"
        );
        assert_eq!(
            acknowledge(
                &bound,
                accepted.unit,
                &[
                    report(1, SubmissionEvidence::Submitted),
                    report(2, SubmissionEvidence::Submitted),
                    report(3, SubmissionEvidence::Submitted),
                    report(4, SubmissionEvidence::Submitted),
                ],
            ),
            rejected,
            "a report for a member the unit does not cover is refused"
        );
        // Nothing moved. A rejection that had already terminalized part of the
        // range could not be taken back, so the check has to precede every
        // mutation rather than merely accompany it.
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![1, 2, 3],
            "a refused acknowledgment resolves nothing and moves no cursor"
        );

        // Mixed reports across one unit are ordinary: a write can submit some
        // members and fail on others, and the unit's shared record cannot express
        // that.
        assert!(
            acknowledge(
                &bound,
                accepted.unit,
                &[
                    report(1, SubmissionEvidence::Submitted),
                    report(2, SubmissionEvidence::NotSubmitted),
                    report(3, SubmissionEvidence::SubmissionUnknown),
                ],
            )
            .is_ok(),
            "a complete set of reports is accepted whatever the reports say"
        );

        // That each member resolves from its own report is asserted at the seam
        // where the value is observable. The acknowledgment above consumes its
        // members, so the outcome it derived for each is not readable from
        // outside; what is readable is that the transition carries the supplied
        // report rather than the unit's shared record, which is the step the
        // acknowledgment relies on.
        let plumbing = "mbx-evidence-plumbing";
        place(plumbing, "mbx-evidence-plumbing-1", 1, mail("body"));
        let plumbing_bound = claim(plumbing);
        let unit = declare(&plumbing_bound, range(1, 1)).expect("declare").unit;
        record_unit_evidence(unit, SubmissionEvidence::Submitted);
        let mut state = lock_ledger().expect("ledger");
        let transition = release_entry(
            &mut state,
            "mbx-evidence-plumbing-1",
            Some(SubmissionEvidence::NotSubmitted),
        );
        assert!(
            matches!(
                transition,
                TerminalTransition::Won {
                    evidence: SubmissionEvidence::NotSubmitted,
                    ..
                }
            ),
            "a member resolves from its own report, not from its unit's record: {transition:?}"
        );
    }
}

/// An acknowledgment resolves exactly the range its declaration bound.
///
/// Inline for the reason given on the peek block above. One test because the
/// claims are one claim seen from both sides: what `ack` resolves and what it
/// leaves alone are the same boundary, and the remainder staying peekable is the
/// only evidence that the cursor advanced by the declared range rather than past
/// everything the executor happened to have peeked.
#[cfg(test)]
mod mailbox_acknowledgment_tests {
    use crate::protocol::mailbox::CursorPosition;

    use super::super::super::super::guard::SubmissionEvidence;

    use super::super::super::terminal::{TerminalTransition, terminalize};
    use super::super::declare::declare;
    use super::super::fixtures::{acknowledge, claim, mail, peeked, place, range, seq};
    use super::*;

    #[test]
    fn an_acknowledgment_resolves_exactly_the_declared_range() {
        let namespace = "mbx-ack";
        let bound = claim(namespace);
        for index in 1..=5 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }

        let accepted = declare(&bound, range(1, 3)).expect("a well-formed range binds");
        let members: Vec<MemberAcknowledgment> = range(1, 3)
            .sequences()
            .map(|sequence| MemberAcknowledgment {
                sequence,
                evidence: SubmissionEvidence::Submitted,
            })
            .collect();
        assert_eq!(
            acknowledge(&bound, accepted.unit, &members),
            Ok(AckAccepted::Terminalized {
                range: range(1, 3),
                cursor: CursorPosition::advanced_through(seq(3)),
            }),
            "the cursor advances by exactly the declared range"
        );

        // The remainder is the assertion with teeth. An acknowledgment that
        // advanced past everything peeked rather than everything declared would
        // leave this empty, and entries 4 and 5 would be lost with no record that
        // anything had happened to them.
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![4, 5],
            "entries outside the declared range stay queued and undeclared"
        );

        // A repeat names a real binding that has already resolved, so it is a
        // no-op rather than a rejection — and it must not advance the cursor a
        // second time.
        assert_eq!(
            acknowledge(&bound, accepted.unit, &members),
            Ok(AckAccepted::AlreadyTerminalized { range: range(1, 3) }),
            "acknowledging a resolved unit again is a no-op"
        );

        // A unit this caller never declared resolves nothing. Minted here rather
        // than invented, so the identifier is well-formed and only its binding is
        // missing.
        assert_eq!(
            acknowledge(&bound, PackingUnitId::mint(), &members),
            Err(AckRejection::UnitNotDeclared),
            "an acknowledgment without a matching declaration is refused"
        );
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![4, 5],
            "a refused acknowledgment advances no cursor"
        );

        // Quota release, observed through the ledger rather than asserted about
        // it: an acknowledged member's reservation is gone, so nothing is left to
        // terminalize, while an unacknowledged one is still held.
        assert_eq!(
            terminalize("mbx-ack-1"),
            TerminalTransition::NoReservation,
            "an acknowledged member's reservation was released"
        );
        assert!(
            matches!(terminalize("mbx-ack-4"), TerminalTransition::Won { .. }),
            "a member outside the acknowledged range still holds its reservation"
        );

        // A second unit, resolved after the first. Re-acknowledging the *earlier*
        // one must still be the no-op it is: it names a real binding that really
        // did resolve, and answering that it was never declared would tell an
        // executor its own completed work never happened. Remembering only the
        // most recent unit is what produces that answer, so this is the assertion
        // that rules it out.
        let later = declare(&bound, range(5, 5)).expect("declare the remaining entry");
        acknowledge(
            &bound,
            later.unit,
            &[MemberAcknowledgment {
                sequence: seq(5),
                evidence: SubmissionEvidence::Submitted,
            }],
        )
        .expect("the later unit resolves");
        assert_eq!(
            acknowledge(&bound, accepted.unit, &members),
            Ok(AckAccepted::AlreadyTerminalized { range: range(1, 3) }),
            "an earlier resolved unit is still remembered once a later one resolves"
        );
    }
}
