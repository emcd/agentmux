//! Issuing the generation entitled to consume a target's mailbox, and replacing
//! it behind a fence.
//!
//! One generation owns a target at a time, and its identifier is drawn from a
//! per-target sequence the relay owns: monotonic, never reused, and never reset
//! by a teardown. That is what a stale identifier is checked against on every
//! peek, declaration, and acknowledgment, so the whole value of the check rests
//! on a superseded identifier being unable to come back around.
//!
//! **The fence runs outside the lock and its verdict is presented here.** The
//! ledger lock is not held across an await, and fence acknowledgment is a
//! bounded observation of another task ceasing — so a caller drives
//! [`acknowledge_fence`](super::super::super::fence::acknowledge_fence) against
//! the outgoing generation first and brings the verdict to the flip. The
//! outgoing generation is named in the same call, so a verdict obtained for a
//! generation that has since been replaced admits nothing: it is rejected as
//! naming a generation that no longer holds the target, rather than being
//! spent on whichever generation happens to be active by then.
//!
//! A generation stops owning a target one of two ways: it is replaced, which is
//! here, or the target goes away, which is [`reap`](super::reap). There is
//! deliberately no bare release beside the reap — giving up ownership and
//! reclaiming what the target held have to happen under one acquisition of the
//! lock, or a consumer could claim the target in between and be handed a mailbox
//! that is reclaimed underneath it.

use crate::protocol::identity::{ConsumerGenerationId, DeliveryTargetId};

use super::super::super::fence::FenceVerdict;
use super::super::ledger::{AdmissionTargetKey, LedgerState, lock_ledger};
use super::addressing::target_key;
use super::resolution::{ResolvedMember, resolve_positions};

/// Why a target's consumer generation was not issued or replaced.
///
/// Names the real cause rather than folding onto a neighbouring one. The peek,
/// declare and acknowledge rejections are protocol shapes a transport sees and
/// coerce a lock failure into `UnknownTarget` for that reason; these are
/// relay-internal, so nothing is leaked by saying what happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum GenerationRejection {
    /// A generation already holds the target. This is the second binding the
    /// single-active-generation rule refuses: a consumer that wants the target
    /// must replace the incumbent behind a fence, not bind beside it.
    AlreadyHeld { active: ConsumerGenerationId },
    /// The generation named as outgoing is not the one holding the target, so
    /// there is nothing for this call to replace. `active` reports what does
    /// hold it, and is `None` when nothing does.
    NotActive {
        active: Option<ConsumerGenerationId>,
    },
    /// The outgoing generation was not observed to cease, so a replacement
    /// might write alongside it. The target is left held by the incumbent,
    /// which is the fail-stop the fence's negative verdict calls for.
    ExecutionNotCeased,
    /// The ledger could not be locked, so nothing was read and nothing changed.
    LedgerUnavailable,
}

/// A replacement the relay admitted.
#[derive(Clone, Debug)]
pub(in crate::relay) struct GenerationReplacement {
    /// The identifier the incoming generation consumes under.
    pub(in crate::relay) generation: ConsumerGenerationId,
    /// Members the replacement terminalized, each owing its sender a terminal
    /// outcome that this call does not emit.
    ///
    /// Returned rather than reported because the ledger resolves and the caller
    /// reports, as every other terminal transition here does: `release_entry`
    /// hands back what a member resolved to and leaves the receipt to whoever
    /// owns the send. Swallowing these would turn a loud outcome into a silent
    /// one for exactly the members most likely to need it — the ones a write
    /// may have half-performed.
    pub(in crate::relay) resolved: Vec<ResolvedMember>,
}

/// Issues the first consumer generation for a target.
///
/// Refuses rather than reissuing when one is already active, because two live
/// consumers for one target is the condition every generation check exists to
/// exclude, and handing out a second identifier would make it representable.
pub(in crate::relay) fn claim_consumer_generation(
    target: &DeliveryTargetId,
) -> Result<ConsumerGenerationId, GenerationRejection> {
    let Ok(mut state) = lock_ledger() else {
        return Err(GenerationRejection::LedgerUnavailable);
    };
    let key = target_key(target);
    // The mailbox is created alongside, so a claimed target answers a peek with
    // an empty run rather than as one the relay has never heard of.
    state.mailboxes.entry(key.clone()).or_default();
    let generations = state.generations.entry(key).or_default();
    if let Some(active) = generations.active {
        return Err(GenerationRejection::AlreadyHeld { active });
    }
    Ok(generations.issue())
}

/// Replaces a target's active consumer generation, given a fence verdict for the
/// outgoing one.
///
/// Runs under the lock `declare` and `ack` take, which is what closes the gap
/// between them: a call bound to the outgoing generation is either fully applied
/// before the flip acquires the lock, or reaches it afterwards and is rejected
/// as superseded. Neither order lets one commit against a generation that has
/// already lost the target.
pub(in crate::relay) fn replace_consumer_generation(
    target: &DeliveryTargetId,
    outgoing: ConsumerGenerationId,
    verdict: FenceVerdict,
) -> Result<GenerationReplacement, GenerationRejection> {
    let Ok(mut state) = lock_ledger() else {
        return Err(GenerationRejection::LedgerUnavailable);
    };
    let key = target_key(target);
    let active = state.generations.get(&key).and_then(|held| held.active);
    if active != Some(outgoing) {
        return Err(GenerationRejection::NotActive { active });
    }
    // Checked after the incumbent is confirmed and before anything mutates, so a
    // negative verdict leaves the target exactly as it found it.
    if verdict != FenceVerdict::Positive {
        return Err(GenerationRejection::ExecutionNotCeased);
    }
    let resolved = resolve_outgoing_declaration(&mut state, &key);
    // The outgoing generation's acknowledgment history goes with it. Its only
    // reader is a repeated acknowledgment from the consumer that made it, and
    // that consumer is the one the fence just observed to cease.
    if let Some(mailbox) = state.mailboxes.get_mut(&key) {
        mailbox.acknowledged.clear();
    }
    // The doorbell is deliberately left alone, under the same rule the reap
    // follows: only a registration displaces a registration. The replacement
    // registers its own as it is constructed, and clearing here would put the
    // flip and that registration into an order that has to be got right — one
    // that registered before flipping would erase its own doorbell, and one
    // arriving late would erase a successor's. The window a clear would close is
    // one where a ring is lost either way, since the fence has just established
    // that the generation the old handle belonged to has ceased.
    let generation = state.generations.entry(key).or_default().issue();
    Ok(GenerationReplacement {
        generation,
        resolved,
    })
}

/// The generation entitled to consume a target's mailbox, or `None` while none
/// is.
///
/// Read through one helper so that every operation checked against it applies
/// the same rule, including the part that is easy to lose at a call site: an
/// unclaimed target matches nothing, rather than matching whatever the caller
/// supplied.
pub(super) fn active_generation(
    state: &LedgerState,
    target: &AdmissionTargetKey,
) -> Option<ConsumerGenerationId> {
    state.generations.get(target).and_then(|held| held.active)
}

/// Terminalizes whatever the outgoing generation had declared and not
/// acknowledged.
///
/// A declared member cannot be left for the incoming generation to peek: the
/// declaration is the record that a write was about to begin, and the fence
/// establishes only that execution has ceased, never whether it took effect
/// first. Re-serving it would risk a second write of a message that may already
/// have landed. The guard's evidence order is what picks the outcome — a unit
/// with no recorded evidence resolves `submission_unknown`, which is the honest
/// reading of a write that may have half-happened.
///
/// Undeclared entries are deliberately untouched: nothing was ever about to
/// write them, so they stay queued and become the incoming generation's to
/// serve.
fn resolve_outgoing_declaration(
    state: &mut LedgerState,
    key: &AdmissionTargetKey,
) -> Vec<ResolvedMember> {
    let Some(outstanding) = state
        .mailboxes
        .get(key)
        .and_then(|mailbox| mailbox.outstanding)
    else {
        return Vec::new();
    };
    // No supplied evidence: a replacement brings no report of its own, so the
    // guard's evidence order is the only source. Resolution retires each
    // member's position as it goes, so the cursor advances over the resolved run
    // rather than parking the incoming generation behind entries nobody will
    // serve.
    let resolved = resolve_positions(
        state,
        key,
        outstanding
            .range
            .sequences()
            .map(|sequence| (sequence, None)),
    );
    if let Some(mailbox) = state.mailboxes.get_mut(key) {
        mailbox.outstanding = None;
    }
    resolved
}

/// A target is owned by one generation, which changes hands only behind a
/// positive fence, and never under an identifier the target has used before.
///
/// Inline for the reason given on the peek block: the ledger is a process-global
/// behind a crate-private lock, and widening these operations to reach them from
/// `tests/` would publish the delivery ledger itself.
///
/// One test rather than several because the ownership rule is a single property
/// and its parts hold each other up. A replacement that supersedes the old
/// binding but restarts the sequence, or one that advances the sequence but
/// leaves the old binding usable, satisfies half of it and defeats the whole
/// point — a stale identifier is only harmless if it is both refused now and
/// unable to come back around later.
#[cfg(test)]
mod consumer_generation_tests {
    use crate::protocol::identity::ConsumerBinding;
    use crate::protocol::operations::{
        AckAccepted, AckRejection, DeclareRejection, MemberAcknowledgment, PeekRejection,
    };

    use super::super::super::super::guard::SubmissionEvidence;
    use super::super::super::terminal::terminalize;
    use super::super::declare::declare;
    use super::super::fixtures::{
        acknowledge, admission_key, binding, claim, mail, peeked, place, range, request, seq,
        target,
    };
    use super::super::peek::peek;
    use super::super::reap::reap_target;
    use super::*;

    #[test]
    fn a_target_changes_generations_only_behind_a_fence_and_never_reuses_one() {
        let namespace = "mbx-generation";
        let first = claim(namespace);

        // No second consumer binds beside the incumbent. This is the whole of
        // the single-active-generation rule at its only entry point: everything
        // downstream is a check against one value, and a second value issued
        // here would make two live consumers representable before any of those
        // checks ran.
        assert_eq!(
            claim_consumer_generation(&target(namespace)),
            Err(GenerationRejection::AlreadyHeld {
                active: first.generation
            }),
            "a target already held issues no second generation"
        );

        for index in 1..=4 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }
        let acknowledged = declare(&first, range(1, 1)).expect("declare");
        acknowledge(
            &first,
            acknowledged.unit,
            &[MemberAcknowledgment {
                sequence: seq(1),
                evidence: SubmissionEvidence::Submitted,
            }],
        )
        .expect("the first unit resolves");
        assert_eq!(
            acknowledge(&first, acknowledged.unit, &[]),
            Ok(AckAccepted::AlreadyTerminalized { range: range(1, 1) }),
            "the incumbent's own repeat acknowledgment is the no-op it is"
        );
        let declared = declare(&first, range(2, 3)).expect("declare");

        // A negative verdict leaves the target held rather than merely returning
        // an error: the incumbent still owns it, and its declaration is still
        // outstanding. Asserting the refusal alone would pass against a
        // replacement that flipped the generation and reported failure anyway.
        assert_eq!(
            replace_consumer_generation(
                &target(namespace),
                first.generation,
                FenceVerdict::Negative
            )
            .err(),
            Some(GenerationRejection::ExecutionNotCeased),
            "a generation not observed to cease is not replaced"
        );
        assert_eq!(
            peeked(&first, 10, 1_000),
            vec![2, 3, 4],
            "the incumbent still owns the target after a refused replacement"
        );

        // A verdict is spent on the generation it was obtained for. Without
        // this, two supervisors that each fenced the incumbent would each admit
        // a replacement, and the second would supersede a generation nothing
        // had ever fenced.
        let stale = binding(namespace, first.generation.value() + 7);
        assert_eq!(
            replace_consumer_generation(
                &target(namespace),
                stale.generation,
                FenceVerdict::Positive
            )
            .err(),
            Some(GenerationRejection::NotActive {
                active: Some(first.generation)
            }),
            "a verdict for a generation that does not hold the target admits nothing"
        );

        let replacement = replace_consumer_generation(
            &target(namespace),
            first.generation,
            FenceVerdict::Positive,
        )
        .expect("a fenced incumbent is replaced");
        let second = ConsumerBinding::new(target(namespace), replacement.generation);
        assert!(
            replacement.generation.value() > first.generation.value(),
            "a replacement generation advances the sequence"
        );

        // What the outgoing generation had declared is resolved here, and
        // reported back rather than swallowed. `submission_unknown` is the
        // guard's reading of a unit that was declared and never reported on:
        // the fence establishes that execution ceased, never whether it took
        // effect first.
        assert_eq!(
            replacement
                .resolved
                .iter()
                .map(|member| (member.message_id.as_str(), member.evidence))
                .collect::<Vec<_>>(),
            vec![
                (
                    format!("{namespace}-2").as_str(),
                    SubmissionEvidence::SubmissionUnknown
                ),
                (
                    format!("{namespace}-3").as_str(),
                    SubmissionEvidence::SubmissionUnknown
                ),
            ],
            "the outgoing generation's declared members are resolved and handed back"
        );
        // Each carries the send it answered for, which is the whole reason the
        // mailbox holds one: the message id names the entry, and only the task
        // names the sender owed an outcome for it.
        assert!(
            replacement
                .resolved
                .iter()
                .all(|member| member.task.message_id == member.message_id),
            "a resolved member carries the send that answers for it"
        );
        assert_eq!(
            declared.range,
            range(2, 3),
            "and they are exactly the run the outgoing generation had declared"
        );

        // The superseded binding is refused by all three operations. Each is
        // asserted rather than one standing for the others, because each reads
        // the generation at its own head and one could be left reading the old
        // field while the others moved.
        assert_eq!(
            peek(&request(&first, 10, 1_000)).unwrap_err(),
            PeekRejection::GenerationSuperseded,
            "a superseded generation peeks nothing"
        );
        assert_eq!(
            declare(&first, range(4, 4)),
            Err(DeclareRejection::GenerationSuperseded),
            "a superseded generation declares nothing"
        );
        assert_eq!(
            acknowledge(&first, declared.unit, &[]),
            Err(AckRejection::GenerationSuperseded),
            "a superseded generation acknowledges nothing"
        );

        // The incoming generation inherits the mailbox but not the outgoing
        // one's acknowledgment history, which went with the consumer that made
        // it. A unit resolved before the replacement is one this caller never
        // declared, rather than one it is told it already acknowledged.
        assert_eq!(
            acknowledge(&second, acknowledged.unit, &[]),
            Err(AckRejection::UnitNotDeclared),
            "the outgoing generation's resolved units are not the incoming one's to see"
        );
        // Undeclared entries stay queued and become the incoming generation's to
        // serve, while the resolved run is behind the cursor rather than parking
        // the mailbox in front of it.
        assert_eq!(
            peeked(&second, 10, 1_000),
            vec![4],
            "the incoming generation serves what was never declared, from a cursor that moved"
        );
        assert!(
            declare(&second, range(4, 4)).is_ok(),
            "and the outgoing generation's declaration no longer blocks its own"
        );
        assert_eq!(
            reserved_envelopes(namespace),
            1,
            "the resolved members released their reservations, leaving only the entry still held"
        );

        // Teardown and recreation. The sequence is the one thing that survives
        // it: a target whose mailbox and cursor are reclaimed and then recreated
        // under the same session name continues the sequence rather than
        // restarting it, so an identifier held from before cannot match one
        // issued after. What the reap itself refuses and reclaims is its own
        // property and is asserted with it; this is only the teardown the
        // sequence has to outlive.
        terminalize(&format!("{namespace}-4"));
        reap_target(&target(namespace), Some(second.generation))
            .expect("the incumbent gives up the target it holds");
        let third = claim_consumer_generation(&target(namespace))
            .expect("a reaped target is free to claim again");
        assert_eq!(
            third.value(),
            replacement.generation.value() + 1,
            "a recreated target continues its sequence rather than restarting it"
        );

        // And a target nobody has claimed is owned by nobody. There is no
        // default generation to guess, which is what makes every check above a
        // check rather than a comparison against a constant every caller knows.
        let unclaimed = "mbx-generation-unclaimed";
        place(unclaimed, "mbx-generation-unclaimed-1", 1, mail("body"));
        assert_eq!(
            peek(&request(&binding(unclaimed, 1), 10, 1_000)).unwrap_err(),
            PeekRejection::GenerationSuperseded,
            "an unclaimed target refuses the caller that guessed the first identifier"
        );
    }

    /// How many envelopes the target still has reserved.
    ///
    /// Read from the ledger because quota release is not otherwise visible from
    /// inside this module, and the mailbox emptying does not imply it: depth and
    /// reservation are separate state released by one transition, so one can
    /// return while the other leaks.
    fn reserved_envelopes(namespace: &str) -> usize {
        let state = lock_ledger().expect("ledger");
        state
            .per_target
            .get(&admission_key(namespace))
            .map_or(0, |usage| usage.envelopes)
    }
}
