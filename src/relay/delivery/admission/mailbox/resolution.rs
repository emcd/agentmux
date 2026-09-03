//! Terminalizing a run of mailbox positions, and carrying back what each of them
//! resolved to.
//!
//! Three paths resolve entries the relay holds rather than entries a caller
//! named: an acknowledgment, a fenced replacement giving up what the outgoing
//! generation had declared, and a target observed unreachable past its dwell.
//! All three run inside one acquisition of the ledger lock, and none of them may
//! report from inside it — reporting emits inscriptions, routes a receipt through
//! another target's worker, and reaches locks of its own, so a resolution taken
//! under the ledger lock has to be handed back and published afterwards.
//!
//! That handing back is what [`ResolvedMember`] is for. It carries the send
//! itself rather than an identifier, because the winning terminal transition
//! removes the ledger entry and takes the mailbox slot with it: by the time a
//! caller could look the member up again, there is nothing left to look up.

use std::sync::Arc;

use crate::protocol::mailbox::EntrySequence;
use crate::relay::AsyncDeliveryTask;

use super::super::super::guard::{GuardKey, SubmissionEvidence};
use super::super::ledger::{AdmissionTargetKey, LedgerState};
use super::super::terminal::{TerminalTransition, release_entry};

/// One entry the ledger terminalized, and what its sender is owed for it.
///
/// Produced only by a path that won the transition. A member another resolver
/// had already taken never appears here, which is what keeps the caller from
/// reporting a duplicate outcome for it.
#[derive(Clone, Debug)]
pub(in crate::relay) struct ResolvedMember {
    pub(in crate::relay) message_id: String,
    /// The send this entry answered for, taken from the mailbox slot before the
    /// transition removed it.
    pub(in crate::relay) task: Arc<AsyncDeliveryTask>,
    /// What the guard's evidence order resolved it to, or what the caller
    /// supplied for it.
    pub(in crate::relay) evidence: SubmissionEvidence,
    /// The position the member was bound under, which the terminal-outcome
    /// inscription reports so a reader can correlate an outcome to a mailbox
    /// position.
    pub(in crate::relay) guard: Option<GuardKey>,
    /// Whether a declaration had bound this entry to a packing unit.
    ///
    /// Reported because one caller spells its members' outcomes by it. Graceful
    /// shutdown resolves an **undeclared** entry `dropped_on_shutdown` — nothing
    /// was ever about to write it, and naming the process exiting is more use to
    /// a sender than the generic non-delivery — while a **declared** one must
    /// take the guard's evidence order instead, because a write may already have
    /// reached the target for it. `evidence` cannot carry the distinction: it
    /// reads `not_submitted` for an undeclared entry either way.
    pub(in crate::relay) declared: bool,
}

/// Terminalizes each named position, collecting what the ones this caller won
/// resolved to.
///
/// `supplied` is per position rather than per call: an acknowledgment reports
/// what the write observed for each member separately, and a unit whose write
/// submitted some members and failed on others cannot be described by one value.
/// `None` runs the guard's evidence order for that position, which is what a
/// lifecycle trigger brings.
///
/// The slot's task is read before the transition rather than after, because the
/// transition retires the position and removes the slot with it.
pub(super) fn resolve_positions(
    state: &mut LedgerState,
    key: &AdmissionTargetKey,
    positions: impl IntoIterator<Item = (EntrySequence, Option<SubmissionEvidence>)>,
) -> Vec<ResolvedMember> {
    let held: Vec<_> = positions
        .into_iter()
        .filter_map(|(sequence, supplied)| {
            let slot = state.mailboxes.get(key)?.slots.get(&sequence)?;
            Some((
                sequence,
                slot.message_id.clone(),
                Arc::clone(&slot.task),
                supplied,
            ))
        })
        .collect();
    held.into_iter()
        .filter_map(|(sequence, message_id, task, supplied)| {
            let (evidence, guard, declared) =
                match release_entry(state, message_id.as_str(), supplied) {
                    TerminalTransition::Won {
                        evidence,
                        guard,
                        bound,
                    } => (evidence, guard, bound),
                    // Relay-originated work holds no reservation, so there is
                    // nothing for the transition to win — and nothing racing it
                    // either, which is what makes reporting safe here where
                    // absence alone would not. The distinction is the task's:
                    // for an *admitted* member an absent reservation means
                    // another resolver already won and cleaned up, and reporting
                    // would be the duplicate the guard exists to prevent.
                    //
                    // Its position is retired here, because the transition that
                    // would otherwise have done it did not happen. Leaving it
                    // would park the cursor on a slot nothing will ever resolve,
                    // and every entry behind it with it.
                    TerminalTransition::NoReservation if !task.admitted => {
                        if let Some(mailbox) = state.mailboxes.get_mut(key) {
                            mailbox.retire(sequence);
                        }
                        (
                            supplied.unwrap_or(SubmissionEvidence::NotSubmitted),
                            None,
                            false,
                        )
                    }
                    TerminalTransition::NoReservation | TerminalTransition::AlreadyTerminal => {
                        return None;
                    }
                };
            Some(ResolvedMember {
                message_id,
                task,
                evidence,
                guard,
                declared,
            })
        })
        .collect()
}
