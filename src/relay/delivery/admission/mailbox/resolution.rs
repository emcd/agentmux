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
            Some((slot.message_id.clone(), Arc::clone(&slot.task), supplied))
        })
        .collect();
    held.into_iter()
        .filter_map(|(message_id, task, supplied)| {
            let TerminalTransition::Won {
                evidence, guard, ..
            } = release_entry(state, message_id.as_str(), supplied)
            else {
                return None;
            };
            Some(ResolvedMember {
                message_id,
                task,
                evidence,
                guard,
            })
        })
        .collect()
}
