//! Placing an admitted entry into its target's mailbox, and naming the
//! generation entitled to consume it.

use serde_json::json;

use crate::protocol::mailbox::{EntrySequence, MailboxPayload};
use crate::runtime::inscriptions::emit_inscription;

use super::super::super::guard::QueueEntryState;
use super::super::ledger::{MailboxSlot, lock_ledger};

const INSCRIPTION_MAILBOX_ENQUEUED: &str = "relay.delivery.mailbox.enqueued";

/// Why an entry could not be placed in its target's mailbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum EnqueueRejection {
    /// No reservation is held under this message id, so there is no position to
    /// fill and no quota backing the entry.
    NotAdmitted,
    /// The entry already occupies its position. Enqueueing is write-once: a
    /// second payload for one position would change what a peek already reported.
    AlreadyEnqueued,
    /// The entry has already terminalized, so its position no longer exists.
    AlreadyTerminal,
}

/// Places an admitted entry's payload at the position admission gave it, making
/// it peekable.
///
/// Separate from admission because the two know different things: admission runs
/// at the request boundary and knows what an entry costs, while the payload a
/// transport is asked to write is settled afterwards. The position is fixed at
/// admission either way, so the order entries become peekable in cannot differ
/// from the order they were admitted in.
pub(in crate::relay) fn enqueue(
    message_id: &str,
    payload: MailboxPayload,
) -> Result<EntrySequence, EnqueueRejection> {
    let Ok(mut state) = lock_ledger() else {
        return Err(EnqueueRejection::NotAdmitted);
    };
    let Some(entry) = state.entries.get(message_id) else {
        return Err(EnqueueRejection::NotAdmitted);
    };
    if entry.state == QueueEntryState::Terminal {
        return Err(EnqueueRejection::AlreadyTerminal);
    }
    let (target, sequence) = (entry.target.clone(), entry.sequence);
    // The reservation this entry was admitted under, read before the mailbox
    // borrow rather than after it because both live on the same guarded state.
    // Admission has already counted this entry, so the figures include it: with
    // one entry outstanding at a time they are one envelope and that envelope's
    // bytes, and a target whose deliveries failed to release would report them
    // climbing with the send count instead.
    let reserved = state.per_target.get(&target).copied().unwrap_or_default();
    // Taken ahead of the mailbox borrow rather than after the insertion, so
    // nothing below has to hold two borrows of the ledger at once. It is an
    // `Arc` clone, and whether it is rung is decided afterwards.
    let registered = state.doorbells.get(&target).cloned();
    let mailbox = state.mailboxes.entry(target.clone()).or_default();
    if mailbox.slots.contains_key(&sequence) {
        return Err(EnqueueRejection::AlreadyEnqueued);
    }
    // Read before the insertion, because a doorbell reports a transition and
    // only the reading taken beforehand establishes one.
    let head_was_peekable = mailbox.head_is_peekable();
    mailbox.slots.insert(
        sequence,
        MailboxSlot {
            message_id: message_id.to_string(),
            payload,
        },
    );
    // Rung when a peek that would have come back empty would now come back with
    // something. Deliberately narrower than "the mailbox gained an entry": an
    // entry filling a position behind one that is admitted and still unfilled
    // leaves every peek returning nothing, so telling a consumer to look would
    // be telling it about a run it cannot see. It is also narrower than "the
    // mailbox was empty", which is the same case read from the other side —
    // that reading rings for the invisible entry and then stays silent for the
    // one that finally exposes it.
    let doorbell = (!head_was_peekable && mailbox.head_is_peekable())
        .then_some(registered)
        .flatten();
    // What the mailbox holds is otherwise invisible from outside the relay, and
    // three things about it have to be observable rather than argued. Its depth
    // must not grow without bound while the push path — which acknowledges
    // nothing — is the only consumer. The reservation behind it must come back:
    // depth and quota are released by the same terminal transition but through
    // separate state, so one can return while the other leaks. And the stamp on
    // the payload this position now holds is what ties the delivered envelope
    // back to the stored one: a write that rebuilt its own envelope would carry
    // a different one.
    //
    // All three are read from the ledger's own state under the same lock as the
    // insertion, so what is reported is the mailbox's contents rather than a
    // later reading of them or a copy of what the caller passed in.
    let payload_created_at = mailbox
        .slots
        .get(&sequence)
        .and_then(|slot| match &slot.payload {
            MailboxPayload::Mail(envelope) => Some(envelope.message.created_at.as_str()),
            // Raw input is written through verbatim and carries no envelope, so
            // there is no stamp to report and none to compare against.
            MailboxPayload::Raw { .. } => None,
        });
    emit_inscription(
        INSCRIPTION_MAILBOX_ENQUEUED,
        &json!({
            "namespace": target.namespace,
            "target_session": target.target_session,
            "message_id": message_id,
            "sequence": sequence.value(),
            "mailbox_depth": mailbox.slots.len(),
            "cursor": mailbox.cursor.value(),
            "target_envelopes_reserved": reserved.envelopes,
            "target_bytes_reserved": reserved.bytes,
            "payload_created_at": payload_created_at,
            "doorbell_rung": doorbell.is_some(),
        }),
    );
    // The one place in this subsystem where the lock is deliberately released
    // before the operation finishes. A doorbell is foreign code the relay does
    // not own, and the ledger lock is a non-reentrant `std::sync::Mutex`, so a
    // doorbell that reached the ledger — directly, or by waking something that
    // does before it returns — would deadlock the whole of delivery. Ringing
    // afterwards costs nothing: the entry is already placed, so a consumer that
    // peeks the instant it is rung finds what it was rung about.
    drop(state);
    if let Some(doorbell) = doorbell {
        doorbell();
    }
    Ok(sequence)
}

/// A position that leaves the mailbox without being acknowledged does not park
/// the ones behind it.
///
/// Inline for the reason given on the peek block above. One test because the two
/// ways a position can leave — a reservation rolled back before its payload
/// arrived, and an entry a lifecycle trigger resolved — are one defect: absence
/// from the mailbox is ambiguous, and every way of producing it has to be told
/// apart from a position still waiting for its payload or the cursor stalls
/// behind it forever.
#[cfg(test)]
mod mailbox_retirement_tests {
    use super::super::super::admit::rollback_admission;
    use super::super::super::terminal::terminalize;
    use super::super::declare::declare;
    use super::super::fixtures::{admit_only, claim, mail, peeked, place, range};
    use crate::protocol::operations::AckAccepted;

    use super::super::ack::ack;
    use super::*;

    #[test]
    fn a_position_abandoned_or_resolved_outside_an_ack_does_not_park_the_mailbox() {
        // A reservation taken and rolled back before its payload was enqueued.
        // Its position is gone, and the entry behind it must still be reachable:
        // the cursor expects the abandoned position, so leaving it merely absent
        // parks every later entry for this target permanently.
        let rolled_back = "mbx-retire-rollback";
        let rollback_bound = claim(rolled_back);
        admit_only(rolled_back, "mbx-retire-rollback-a", 1);
        admit_only(rolled_back, "mbx-retire-rollback-b", 1);
        rollback_admission("mbx-retire-rollback-a");
        enqueue("mbx-retire-rollback-b", mail("body")).expect("enqueue");
        assert_eq!(
            peeked(&rollback_bound, 10, 1_000),
            vec![2],
            "an entry behind an abandoned position is still peekable"
        );
        assert!(
            declare(&rollback_bound, range(2, 2)).is_ok(),
            "the cursor moved over the abandoned position, so the entry behind it can be declared"
        );

        // An entry resolved by a lifecycle trigger rather than by an
        // acknowledgment. Its slot must go with its reservation, or the head of
        // the mailbox names an entry the ledger no longer holds.
        let triggered = "mbx-retire-trigger";
        let trigger_bound = claim(triggered);
        for index in 1..=3 {
            place(triggered, &format!("{triggered}-{index}"), 1, mail("body"));
        }
        terminalize("mbx-retire-trigger-1");
        assert_eq!(
            peeked(&trigger_bound, 10, 1_000),
            vec![2, 3],
            "a member resolved outside an acknowledgment leaves the head clear"
        );

        // The same trigger against a *declared* member. Its declaration then
        // describes nothing, and leaving it outstanding would refuse every later
        // declaration for this target with a unit no one can ever acknowledge.
        let declared = declare(&trigger_bound, range(2, 2)).expect("declare");
        terminalize("mbx-retire-trigger-2");
        assert_eq!(
            declare(&trigger_bound, range(3, 3)).map(|accepted| accepted.range),
            Ok(range(3, 3)),
            "a declaration whose members were all resolved elsewhere stops blocking"
        );
        // And the executor that acknowledges it afterwards is told what actually
        // happened — already resolved — rather than that it never declared it.
        assert_eq!(
            ack(&trigger_bound, declared.unit, &[]),
            Ok(AckAccepted::AlreadyTerminalized { range: range(2, 2) }),
            "the abandoned declaration is remembered as resolved, not as never made"
        );
    }
}
