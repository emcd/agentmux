//! A stub mailbox a delivery-loop executor can be driven against.
//!
//! The relay's own mailbox is `pub(in crate::relay)` and reaching it from a test
//! would mean publishing the delivery ledger, so what a test drives instead is
//! the seam the transports actually hold: an `Arc<dyn MailboxConsumer>`. This
//! stub answers it with the parts of the contract an executor's behavior depends
//! on — a repeatable head-run peek, a raw entry returned alone, a declaration
//! that must precede an acknowledgment, and a cursor that advances only on one —
//! and records what the executor did so a test can assert on it.
//!
//! Deliberately not a reimplementation of the ledger. It enforces nothing the
//! relay enforces about generations or quota, because a test driving an executor
//! is asking what the *executor* does, and a stub that refused calls on its own
//! terms would answer a different question.
//!
//! Shared by both harnesses through `#[path]`, so a change to the contract breaks
//! one fixture rather than two that had drifted apart.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use agentmux::protocol::mailbox::{CursorPosition, EntryRange, EntrySequence, MailboxEntry};
use agentmux::protocol::operations::{
    AckAccepted, AckRejection, AckResult, DeclareAccepted, DeclareRejection, DeclareResult,
    MemberAcknowledgment, PeekResponse, PeekResult,
};
use agentmux::transports::{MailboxConsumer, PackingUnitId, SubmissionEvidence};

/// What one acknowledged member reported, in the order the executor reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckedMember {
    pub sequence: EntrySequence,
    pub message_id: String,
    pub evidence: SubmissionEvidence,
}

#[derive(Default)]
struct StubState {
    queued: VecDeque<MailboxEntry>,
    cursor: CursorPosition,
    outstanding: Option<(PackingUnitId, EntryRange)>,
    acked: Vec<AckedMember>,
    /// The positions each acknowledged unit covered, one entry per unit.
    ///
    /// Distinct from `acked`, and the distinction matters: `acked` flattens every
    /// member, so it cannot say whether two members shared a write or were
    /// written separately — which is exactly the question a transport that
    /// coalesces has to be held to. Recorded from the declaration's range rather
    /// than from the reports, because the range is what the relay bound.
    acked_units: Vec<Vec<EntrySequence>>,
    /// Every peek the executor made, as the run it was shown. Retained because a
    /// peek that returned nothing is evidence too: it is how "the executor was
    /// running and had nothing to write" is told apart from "the executor never
    /// ran".
    peeks: Vec<Vec<EntrySequence>>,
    unreachable_resolutions: usize,
}

/// A mailbox holding a fixed run of entries, consumed by whichever executor is
/// handed it.
pub struct StubMailbox {
    state: Mutex<StubState>,
}

impl StubMailbox {
    /// Seeds a mailbox whose entries occupy positions 1..=n in the order given.
    #[must_use]
    pub fn with_entries(entries: Vec<MailboxEntry>) -> Self {
        Self {
            state: Mutex::new(StubState {
                queued: entries.into(),
                cursor: CursorPosition::start(),
                ..StubState::default()
            }),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::with_entries(Vec::new())
    }

    /// Places an entry behind whatever the mailbox already holds, after
    /// construction.
    ///
    /// Seeding through [`with_entries`](Self::with_entries) puts every entry
    /// there before any executor runs, which cannot express an arrival while one
    /// is already idle — the case a doorbell exists to shorten and a bounded poll
    /// exists to survive without.
    ///
    /// Deliberately silent: this is the relay-side write, and the ring is a
    /// separate signal the caller makes or withholds for itself. A push that rang
    /// would leave no way to model the ring being lost.
    pub fn place(&self, entry: MailboxEntry) {
        self.state
            .lock()
            .expect("stub mailbox")
            .queued
            .push_back(entry);
    }

    /// What the executor acknowledged, in order.
    #[must_use]
    pub fn acked(&self) -> Vec<AckedMember> {
        self.state.lock().expect("stub mailbox").acked.clone()
    }

    /// The head runs the executor was shown, in order.
    #[must_use]
    pub fn peeks(&self) -> Vec<Vec<EntrySequence>> {
        self.state.lock().expect("stub mailbox").peeks.clone()
    }

    /// The positions each acknowledged packing unit covered, in order.
    #[must_use]
    pub fn acked_units(&self) -> Vec<Vec<EntrySequence>> {
        self.state.lock().expect("stub mailbox").acked_units.clone()
    }

    /// Whether every seeded entry has been acknowledged.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        self.state.lock().expect("stub mailbox").queued.is_empty()
    }

    /// The range a declaration currently binds, if one is outstanding.
    ///
    /// "Undeclared" is otherwise not observable from outside: an entry that was
    /// never declared and one that was declared and acknowledged both leave no
    /// trace in `acked`, and only this tells them apart. A test asserting that a
    /// transport left entries alone needs to say they were never bound, not
    /// merely that they were never acknowledged.
    #[must_use]
    pub fn outstanding_range(&self) -> Option<EntryRange> {
        self.state
            .lock()
            .expect("stub mailbox")
            .outstanding
            .map(|(_, range)| range)
    }

    #[must_use]
    pub fn unreachable_resolutions(&self) -> usize {
        self.state
            .lock()
            .expect("stub mailbox")
            .unreachable_resolutions
    }
}

impl MailboxConsumer for StubMailbox {
    fn peek(&self, entry_max: usize, canonical_bytes_max: u64) -> PeekResult {
        let mut state = self.state.lock().expect("stub mailbox");
        let mut entries: Vec<MailboxEntry> = Vec::new();
        let mut bytes = 0u64;
        for entry in &state.queued {
            if entries.len() >= entry_max {
                break;
            }
            // A raw entry is a barrier in both directions: it is never combined
            // with mail, and mail behind it is not reachable until it is
            // acknowledged. Both halves are load-bearing for the ordering an
            // executor is trusted to preserve, so the stub enforces them rather
            // than handing out a run production could not produce.
            if entry.is_barrier() {
                if entries.is_empty() {
                    entries.push(entry.clone());
                }
                break;
            }
            // The head entry is returned whatever it costs. A mailbox that
            // withheld an oversized head would park every entry behind it
            // forever.
            if !entries.is_empty() && bytes + entry.canonical_bytes > canonical_bytes_max {
                break;
            }
            bytes += entry.canonical_bytes;
            entries.push(entry.clone());
        }
        state
            .peeks
            .push(entries.iter().map(|entry| entry.sequence).collect());
        let cursor = state.cursor;
        Ok(PeekResponse { entries, cursor })
    }

    fn declare(&self, range: EntryRange) -> DeclareResult {
        let mut state = self.state.lock().expect("stub mailbox");
        if let Some((outstanding, _)) = state.outstanding {
            return Err(DeclareRejection::UnitAlreadyOutstanding { outstanding });
        }
        let expected = state.cursor.next_sequence();
        if range.from() != expected {
            return Err(DeclareRejection::NotAtCursor {
                expected,
                requested: range.from(),
            });
        }
        let unit = PackingUnitId::mint();
        state.outstanding = Some((unit, range));
        Ok(DeclareAccepted { unit, range })
    }

    fn ack(&self, unit: PackingUnitId, members: &[MemberAcknowledgment]) -> AckResult {
        let mut state = self.state.lock().expect("stub mailbox");
        let Some((outstanding, range)) = state.outstanding else {
            return Err(AckRejection::UnitNotDeclared);
        };
        if outstanding != unit {
            return Err(AckRejection::UnitNotDeclared);
        }
        let covered: Vec<EntrySequence> = range.sequences().collect();
        if members.len() != covered.len()
            || !covered
                .iter()
                .all(|sequence| members.iter().any(|member| member.sequence == *sequence))
        {
            return Err(AckRejection::EvidenceDoesNotCoverUnit { expected: range });
        }
        for member in members {
            let message_id = state
                .queued
                .iter()
                .find(|entry| entry.sequence == member.sequence)
                .map(|entry| entry.message_id.clone())
                .unwrap_or_default();
            state.acked.push(AckedMember {
                sequence: member.sequence,
                message_id,
                evidence: member.evidence,
            });
        }
        state.acked_units.push(covered.clone());
        state
            .queued
            .retain(|entry| !covered.contains(&entry.sequence));
        state.cursor = CursorPosition::advanced_through(range.through());
        state.outstanding = None;
        Ok(AckAccepted::Terminalized {
            range,
            cursor: state.cursor,
        })
    }

    fn resolve_unreachable(&self) {
        let mut state = self.state.lock().expect("stub mailbox");
        state.unreachable_resolutions += 1;
        // Undeclared entries are what the relay resolves here, so they leave the
        // mailbox; a declared one stays for its acknowledgment.
        let declared: Vec<EntrySequence> = state
            .outstanding
            .map(|(_, range)| range.sequences().collect())
            .unwrap_or_default();
        state
            .queued
            .retain(|entry| declared.contains(&entry.sequence));
    }
}
