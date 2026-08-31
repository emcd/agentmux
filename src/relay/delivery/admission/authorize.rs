//! The all-or-none batch transition, packing units, and the write-once evidence
//! every member of a unit resolves from.

use std::collections::HashSet;

use serde_json::json;

use crate::runtime::inscriptions::emit_inscription;
use crate::transports::PartitionError;

use super::super::guard::{BatchId, GuardKey, PackingUnitId, QueueEntryState, SubmissionEvidence};
use super::ledger::{LedgerState, UnitRecord, lock_ledger};

/// Authorizes a whole batch, minting the batch's identity and creating every
/// member's guard in one atomic operation.
///
/// This is the model's sole linearization point. It is a relay-local state
/// transition on the relay's own queue entries — not a call, not a handshake, and
/// not dependent on any transport observing anything — which is why it is
/// trivially atomic and why there is no acceptance race to adjudicate.
///
/// **All members or none.** A batch is the unit of authorization and there is no
/// partially-authorized batch, so the whole set is validated under the lock
/// before any entry moves. Guarding the members that happened to be
/// unauthorized and reporting success would produce exactly that forbidden state:
/// the relay would go on to submit a set whose membership no longer matches the
/// one it authorized, which is the mutable membership that let one outcome be
/// reported for members that were written and members that were not.
///
/// One `BatchId` names the whole set. A member carries no identity of its own
/// beyond its mailbox position, because acknowledgment is idempotent per entry:
/// a second acknowledgment of a member already resolved is a no-op rather than a
/// distinct attempt to be told apart from the first.
///
/// A member that already holds a guard vetoes the batch and yields `None`, as
/// does a member the ledger does not know: authorization is irrevocable, so a
/// second one would be a second attempt at a member the relay has already
/// committed. Holding a guard is what "already committed" means — it is assigned
/// exactly when something takes responsibility for delivering the entry, and
/// never reassigned.
pub(in crate::relay) fn authorize_batch(member_ids: &[&str]) -> Option<BatchId> {
    // Shape first, above the lock, for the same reason `declare_packing_unit`
    // checks it there: neither check reads ledger state. A duplicate would mint
    // two attempts against one entry and leave only the second recorded, so the
    // relay would believe it had authorized more members than it holds.
    if member_ids.is_empty() {
        return None;
    }
    let unique: HashSet<&str> = member_ids.iter().copied().collect();
    if unique.len() != member_ids.len() {
        return None;
    }
    let mut state = lock_ledger().ok()?;
    let all_unauthorized = member_ids.iter().all(|id| {
        state
            .entries
            .get(*id)
            .is_some_and(|entry| entry.state == QueueEntryState::Queued && entry.guard.is_none())
    });
    if !all_unauthorized {
        return None;
    }
    let batch = BatchId::mint();
    let sequences: Vec<_> = member_ids
        .iter()
        .filter_map(|id| state.entries.get(*id).map(|entry| entry.sequence))
        .collect();
    for (id, sequence) in member_ids.iter().zip(sequences) {
        if let Some(entry) = state.entries.get_mut(*id) {
            entry.guard = Some(GuardKey::new(sequence));
        }
    }
    Some(batch)
}

/// Mints a packing unit and binds every proposed member to it, or binds none.
///
/// Called before the unit produces any target-side effect. Until a member is
/// bound the guard can positively prove nothing was written; after it, partial
/// effect cannot be excluded. Recording the binding ahead of the effect is what
/// keeps a lifecycle trigger from over-claiming in either direction, and it is
/// why the evidence order reads binding rather than asking how the attempt ended.
///
/// **The validation has to happen inside this lock, and the refusal has to be a
/// refusal.** The tempting alternative — bind whoever is still bindable and
/// return the id anyway — lets a member that terminalized a moment earlier
/// resolve `not_submitted`, a *positive* claim that nothing was written, while
/// the transport goes on to write the group it had already composed. Binding
/// after declaration is harmless by comparison: the evidence order falls back to
/// `submission_unknown` once a member is bound, so a late binding costs precision
/// rather than correctness. The asymmetry is the whole reason for the `Result`.
///
/// A member that vetoes therefore vetoes its **whole** proposed unit, and its
/// groupmates resolve `not_submitted` because no effect occurred for them either.
/// That is an accepted cost, not an oversight: declaring the bindable subset was
/// considered and rejected, because a transport handed a partial unit would have
/// to re-derive which members its already-composed payload actually covers, and
/// getting that wrong reintroduces exactly the false-non-delivery this prevents.
///
/// Binding is write-once, so a member already bound is not bindable: a second
/// bind would mean the partition changed after it was recorded, which the
/// contract forbids.
pub(in crate::relay) fn declare_packing_unit(
    member_ids: &[&str],
) -> Result<PackingUnitId, PartitionError> {
    // Shape first, above the lock: the slice is immutable for this synchronous
    // call and neither check reads ledger state, so nothing here can go stale
    // before the binding below. Both malformed shapes corrupt the record's member
    // count rather than the binding itself — and the count is what decides when
    // the record may be dropped. A
    // duplicate binds one entry once but counts it twice, so the single
    // terminalization never reaches zero and the record outlives the process. An
    // empty declaration mints a unit no member will ever terminalize, with the
    // same result. Neither is reachable from Tmux's current group construction;
    // that is a fact about today's caller, not a property of the boundary.
    if member_ids.is_empty() {
        return Err(PartitionError::MemberNotBindable);
    }
    let unique: HashSet<&str> = member_ids.iter().copied().collect();
    if unique.len() != member_ids.len() {
        return Err(PartitionError::MemberNotBindable);
    }
    let Ok(mut state) = lock_ledger() else {
        return Err(PartitionError::LedgerUnavailable);
    };
    let all_bindable = member_ids.iter().all(|id| {
        state.entries.get(*id).is_some_and(|entry| {
            entry.state == QueueEntryState::Queued && entry.guard.is_some() && entry.unit.is_none()
        })
    });
    if !all_bindable {
        return Err(PartitionError::MemberNotBindable);
    }
    let unit = PackingUnitId::mint();
    for id in member_ids {
        if let Some(entry) = state.entries.get_mut(*id) {
            entry.unit = Some(unit);
        }
    }
    state.units.insert(
        unit,
        UnitRecord {
            evidence: None,
            unresolved_members: member_ids.len(),
        },
    );
    // The partition is otherwise invisible. Every other step of a delivery leaves
    // a record, but which members shared a fate — the thing that decides whose
    // outcome is derived from whose evidence — could not be answered from the log
    // at all. That is a gap in an arc whose subject is per-member attribution: a
    // reader could see two members resolve identically without being able to tell
    // whether that was one record answering for both or two records agreeing.
    emit_inscription(
        "relay.delivery.partition.declared",
        &json!({
            "unit_id": unit.value(),
            "member_ids": member_ids,
            "member_count": member_ids.len(),
        }),
    );
    Ok(unit)
}

/// Records the immutable evidence for a packing unit, before any member outcome
/// is derived from it.
///
/// Written once; a later record is ignored, because the first one is what any
/// resumed fan-out must agree with. A record for a unit whose members have all
/// terminalized is dropped rather than resurrected — there is no one left to
/// resolve from it.
pub(in crate::relay) fn record_unit_evidence(unit: PackingUnitId, evidence: SubmissionEvidence) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    write_unit_evidence(&mut state, unit, evidence);
}

/// Records evidence against whatever unit the member is bound to.
///
/// The relay's own outcome path knows which member resolved, not which unit
/// carried it, and it stays that way once transports declare their own units:
/// the id then belongs to the transport, and the relay would have no way to name
/// it. A member with no unit records nothing — it was never submitted, and the
/// evidence order already resolves it from that fact.
///
/// The write is the same write-once one, so a transport that recorded through
/// its sink during the write has already won and this call changes nothing.
pub(in crate::relay) fn record_evidence_for_member(message_id: &str, evidence: SubmissionEvidence) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    let Some(unit) = state.entries.get(message_id).and_then(|entry| entry.unit) else {
        return;
    };
    write_unit_evidence(&mut state, unit, evidence);
}

fn write_unit_evidence(state: &mut LedgerState, unit: PackingUnitId, evidence: SubmissionEvidence) {
    if let Some(record) = state.units.get_mut(&unit)
        && record.evidence.is_none()
    {
        record.evidence = Some(evidence);
    }
}

/// A batch authorization binds every member or none, and a veto leaves the
/// ledger exactly as it found it.
///
/// Inline for the same reason the declaration test below is: the ledger is a
/// process-global whose transitions happen under one crate-private lock, and no
/// public interface can drive a *multi-member* authorization — the relay forms a
/// batch of one per invocation, so the veto this pins has no reachable trigger
/// from outside. The single-member case a send does exercise cannot discriminate
/// all-or-none from bind-what-you-can, because with one member the two agree.
///
/// What it pins is the refusal, because the refusal is what keeps the relay from
/// writing an unauthorized member. Binding the still-unauthorized siblings and
/// reporting success would produce a partially-authorized batch — membership
/// that changed after the relay committed, which is the mutable membership the
/// contract forbids by name.
#[cfg(test)]
mod batch_authorization_tests {
    use super::*;
    use crate::configuration::SessionType;
    use std::path::Path;

    use super::super::admit::admit;
    use super::super::ledger::AdmissionTargetKey;

    #[test]
    fn a_veto_leaves_every_sibling_unauthorized() {
        let target = AdmissionTargetKey::new(
            "batch-auth-test",
            Path::new("/nonexistent/batch-auth-test"),
            "target",
        );
        let bindable = "batch-auth-bindable";
        let vetoing = "batch-auth-vetoing";
        for id in [bindable, vetoing] {
            admit(id, target.clone(), SessionType::Tmux, 1).expect("admit");
        }
        // The veto: one member already holds a guard, which is the state a
        // second authorization of the same member would find.
        authorize_batch(&[vetoing]).expect("the first authorization succeeds");

        assert!(
            authorize_batch(&[bindable, vetoing]).is_none(),
            "a member that already holds a guard vetoes its whole batch"
        );
        // The sibling is the assertion with teeth. Moving the transition into the
        // validation loop would leave it `Authorized` here despite the refusal,
        // which is the partially-authorized batch the contract excludes.
        assert!(
            authorize_batch(&[bindable]).is_some(),
            "the vetoed batch left its bindable sibling still unauthorized"
        );

        // An unknown member vetoes too, and for the same reason: the relay cannot
        // commit to delivering something its ledger has never admitted.
        let fresh = "batch-auth-fresh";
        admit(fresh, target, SessionType::Tmux, 1).expect("admit");
        assert!(
            authorize_batch(&[fresh, "batch-auth-never-admitted"]).is_none(),
            "an unadmitted member vetoes its whole batch"
        );
        assert!(
            authorize_batch(&[fresh]).is_some(),
            "the unknown member's batch left its sibling still unauthorized"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::SessionType;
    use std::path::Path;

    use super::super::admit::admit;
    use super::super::ledger::AdmissionTargetKey;
    use super::super::terminal::{TerminalTransition, terminalize};

    /// The all-or-nothing declaration is the invariant this whole partition
    /// mechanism rests on, and it is crate-private by design: the ledger is a
    /// process-global holding the one lock under which binding happens, and no
    /// public interface can *drive* a multi-member unit. One is reachable — a
    /// transport declares over whatever group it coalesced — but not
    /// deterministically: the tmux delivery thread drains only what is already
    /// queued and flushes as soon as the channel reports empty, with no coalesce
    /// wait, so whether a second envelope joins is a race between the relay's
    /// submit and that drain. Batch formation does not change this; a batch is the
    /// unit of authorization and a packing unit the unit of submission.
    ///
    /// What it pins is the refusal, because the refusal is what makes a claim of
    /// non-delivery safe. A partial bind would let one member resolve
    /// `not_submitted` — a positive claim nothing was written — while its
    /// groupmates ride a write that did happen.
    #[test]
    fn a_declaration_binds_every_member_or_none() {
        let target = AdmissionTargetKey::new(
            "partition-test",
            Path::new("/nonexistent/partition-test"),
            "target",
        );
        let bindable = "partition-test-bindable";
        let terminal = "partition-test-terminal";
        for id in [bindable, terminal] {
            admit(id, target.clone(), SessionType::Tmux, 1).expect("admit");
        }
        authorize_batch(&[bindable, terminal]).expect("authorize");
        // One member leaves the set of bindable members behind its groupmate's
        // back, which is the race the lock exists to adjudicate.
        terminalize(terminal);

        assert_eq!(
            declare_packing_unit(&[bindable, terminal]),
            Err(PartitionError::MemberNotBindable),
        );
        // A malformed member list is refused for a different reason than an
        // ineligible member, and the reason matters: both shapes would leave the
        // unit's record with a member count no sequence of terminalizations can
        // bring to zero, leaking it for the process lifetime. Neither is
        // reachable from today's Tmux group construction, which is exactly why
        // the boundary rather than the caller has to reject them.
        assert_eq!(
            declare_packing_unit(&[]),
            Err(PartitionError::MemberNotBindable),
        );
        assert_eq!(
            declare_packing_unit(&[bindable, bindable]),
            Err(PartitionError::MemberNotBindable),
        );
        // The survivor stays unbound, so the guard can still prove nothing was
        // written for it rather than falling back to `submission_unknown`.
        assert!(matches!(
            terminalize(bindable),
            TerminalTransition::Won {
                evidence: SubmissionEvidence::NotSubmitted,
                ..
            },
        ));
    }
}

/// One evidence record answers for every sibling bound to its unit.
///
/// Its own block rather than an addition to the module above, which already
/// carries a test. Inline for the same reason that one is: no public interface
/// can *drive* a multi-member unit — one is reachable, since a transport declares
/// over whatever group it coalesced, but only opportunistically, because the tmux
/// delivery thread drains what is already queued and flushes on an empty channel
/// with no coalesce wait. A test asserting one would be asserting a race. The
/// ledger is also deliberately
/// `pub(in crate::relay)` — the public seam for transports is `PartitionSink`,
/// and widening `declare_packing_unit`/`terminalize` to reach them from a test
/// would publish the delivery ledger itself.
///
/// The property is *identity* of outcome across siblings, not agreement. Values
/// that merely happen to match are what the record replaced: before it, each
/// member was resolved from the value its own fan-out branch computed, so
/// sibling disagreement was unrepresentable only by the discipline of the
/// branch. Reading one record makes it unrepresentable structurally.
#[cfg(test)]
mod sibling_agreement_tests {
    use super::*;
    use crate::configuration::SessionType;
    use std::path::Path;

    use super::super::admit::admit;
    use super::super::ledger::AdmissionTargetKey;
    use super::super::terminal::{TerminalTransition, terminalize};

    #[test]
    fn every_sibling_of_a_unit_resolves_from_the_one_recorded_evidence() {
        let target = AdmissionTargetKey::new(
            "sibling-test",
            Path::new("/nonexistent/sibling-test"),
            "target",
        );
        let members = ["sibling-a", "sibling-b", "sibling-c"];
        for id in members {
            admit(id, target.clone(), SessionType::Tmux, 1).expect("admit");
        }
        authorize_batch(&members).expect("authorize");

        let unit = declare_packing_unit(&members).expect("a fully bindable set binds");
        record_unit_evidence(unit, SubmissionEvidence::Submitted);
        // Write-once, asserted through the outcome rather than the record: a
        // resumed fan-out that recorded again must not be able to move an outcome
        // a sibling has already been resolved from. `NotSubmitted` is chosen
        // deliberately over another `Submitted` — it is the one value whose
        // leaking through would turn a delivered member into a positive claim
        // that nothing was written.
        record_unit_evidence(unit, SubmissionEvidence::NotSubmitted);

        // Ordering matters: the last sibling is the one whose read races its
        // unit's release, because that terminalization brings `unresolved_members`
        // to zero and drops the record. It resolves from the same evidence only
        // because `terminalize` reads before it decrements.
        for id in members {
            let transition = terminalize(id);
            let TerminalTransition::Won {
                evidence, bound, ..
            } = transition
            else {
                panic!("sibling {id} did not win its terminal transition: {transition:?}");
            };
            assert_eq!(
                (evidence, bound),
                (SubmissionEvidence::Submitted, true),
                "sibling {id} resolved from something other than its unit's record",
            );
        }
    }
}
