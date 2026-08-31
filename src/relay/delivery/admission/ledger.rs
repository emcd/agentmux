//! The process-global reservation ledger and the one lock that guards it.
//!
//! Every event in this subsystem — admission, authorization, binding, evidence,
//! the terminal transition, and the reporting pass — mutates this state under
//! [`lock_ledger`], acquired exactly once at the head of the entry point and held
//! for the whole operation.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

use crate::protocol::{
    identity::ConsumerGenerationId,
    mailbox::{CursorPosition, EntryRange, EntrySequence, MailboxPayload},
};
use crate::relay::{RelayError, relay_error};

use super::super::guard::{GuardKey, PackingUnitId, QueueEntryState, SubmissionEvidence};

/// Identifies the target a queue entry is admitted against. Same three
/// components as the delivery worker's key, kept as its own type so the ledger
/// does not depend on worker registration: quota is reserved at the request
/// boundary, which can precede the target's worker existing.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(in crate::relay) struct AdmissionTargetKey {
    pub(in crate::relay) namespace: String,
    pub(in crate::relay) runtime_directory: PathBuf,
    pub(in crate::relay) target_session: String,
}

impl AdmissionTargetKey {
    pub(in crate::relay) fn new(
        namespace: &str,
        runtime_directory: &Path,
        target_session: &str,
    ) -> Self {
        Self {
            namespace: namespace.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            target_session: target_session.to_string(),
        }
    }
}

/// One admitted entry: its reservation, its lifecycle state, and the evidence
/// its guard resolves from. Held until the entry terminalizes.
///
/// State and reservation live on the same record, under the same lock, because
/// the terminal transition and the quota release are one atomic operation. Split
/// across two structures they could drift, and a released reservation on a
/// non-terminal entry is exactly the double-resolution this guard exists to make
/// impossible.
#[derive(Clone, Debug)]
pub(super) struct AdmittedEntry {
    pub(super) target: AdmissionTargetKey,
    pub(super) canonical_bytes: u64,
    /// When the entry was admitted, which is what the undelivered-queue warning
    /// measures. It is a report of how long the relay has been waiting and never
    /// an input to resolution: no code path compares it against a bound to decide
    /// an outcome.
    pub(super) admitted_at: Instant,
    /// This entry's position in its target's mailbox, assigned at admission and
    /// never reassigned.
    ///
    /// Assigned here rather than when the entry becomes peekable because
    /// admission is what establishes a target's ordering: two sends that both
    /// reserve quota have already been linearized against each other by the time
    /// either returns, and deferring the number would leave that order to be
    /// re-derived later from something weaker.
    pub(super) sequence: EntrySequence,
    pub(super) state: QueueEntryState,
    /// The guard this entry resolves under, assigned when something takes
    /// responsibility for delivering it and never reassigned; `None` while the
    /// entry is merely waiting.
    pub(super) guard: Option<GuardKey>,
    /// The packing unit this member was partitioned into, recorded before the
    /// unit's first target-side effect and never reassigned. It is the
    /// discriminator in the guard's evidence order between a provable
    /// `not_submitted` and an honest `submission_unknown` — binding, rather than
    /// the manner of the failure that ended the attempt.
    ///
    /// One member per unit today, because the relay submits one member per batch
    /// and each transport coalesces internally without reporting its partition
    /// back. Transport-reported partitions replace the minting site, not this
    /// field or the order that reads it.
    pub(super) unit: Option<PackingUnitId>,
}

/// Per-target usage. An entry is one envelope, so the count is the number of
/// live entries for that target.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TargetUsage {
    pub(super) envelopes: usize,
    pub(super) bytes: u64,
    /// Set when this target's first-crossing warning has been emitted, so a
    /// backlogged target warns once rather than once per queued message.
    ///
    /// Re-arming is structural rather than a separate reset: the whole usage
    /// record is dropped when the target's last entry terminalizes, so a target
    /// that drains and re-accumulates warns again because its flag went with it.
    pub(super) warned: bool,
}

/// One packing unit's shared record.
///
/// The evidence is unit-owned rather than copied into each member: a transport
/// submits a unit, not a member, so one submission produces exactly one result
/// and every member bound to that unit must resolve from the same one. Copying
/// it per member would make disagreement representable, which is the split
/// outcome the guard exists to prevent.
#[derive(Clone, Copy, Debug)]
pub(super) struct UnitRecord {
    /// Written once, before any member outcome is derived from it. A later record
    /// is ignored, because the first one is what any resumed fan-out must agree
    /// with.
    pub(super) evidence: Option<SubmissionEvidence>,
    /// How many bound members have yet to terminalize. The record is dropped when
    /// this reaches zero, so the map tracks live units rather than growing by one
    /// entry per unit the relay ever submitted.
    pub(super) unresolved_members: usize,
}

/// One peekable position in a target's mailbox.
///
/// Holds only what a peek needs that the entry's reservation record does not
/// already carry: which message occupies the position, and what a transport is
/// being asked to write there. Size and admission time are deliberately not
/// duplicated here — they live on [`AdmittedEntry`], which stays the one
/// authoritative record for everything about an entry, so a peek's byte
/// accounting cannot drift from the reservation it was charged against.
#[derive(Clone, Debug)]
pub(super) struct MailboxSlot {
    pub(super) message_id: String,
    pub(super) payload: MailboxPayload,
}

/// A declaration that has been made and not yet acknowledged.
///
/// At most one exists per target at any time, which is what makes two packing
/// units binding the same entry unrepresentable rather than merely unlikely: the
/// cursor does not move until an acknowledgment, so two declarations of the same
/// range would otherwise both pass every position and contiguity check.
#[derive(Clone, Copy, Debug)]
pub(super) struct OutstandingDeclaration {
    pub(super) unit: PackingUnitId,
    pub(super) range: EntryRange,
    /// The generation the declaration was made under. An acknowledgment from a
    /// later generation must not resolve it: the binding belonged to a consumer
    /// that no longer owns the target.
    pub(super) generation: ConsumerGenerationId,
}

/// One target's ordered mailbox: what it holds, how far it has been
/// acknowledged, and who is entitled to consume it.
#[derive(Debug)]
pub(super) struct TargetMailbox {
    /// Peekable positions in mailbox order.
    ///
    /// A position exists here from the moment its payload is enqueued until it
    /// terminalizes. An admitted entry that has not been enqueued has a sequence
    /// number but no slot, and is therefore not peekable — the state a send
    /// occupies between reserving its quota and having a payload to be written.
    pub(super) slots: BTreeMap<EntrySequence, MailboxSlot>,
    pub(super) cursor: CursorPosition,
    /// The next position admission will hand out for this target.
    pub(super) next_sequence: EntrySequence,
    pub(super) outstanding: Option<OutstandingDeclaration>,
    /// Positions this target will never serve.
    ///
    /// A position leaves the mailbox either by being acknowledged, which moves
    /// the cursor past it, or by disappearing some other way: a reservation
    /// rolled back before its payload was ever enqueued, or an entry
    /// terminalized by a lifecycle trigger. This records the second kind.
    ///
    /// It exists because absence from `slots` is ambiguous on its own. A position
    /// that has been admitted but not yet enqueued is also absent, and that one
    /// *will* be filled — so the cursor must wait for it. Without a record
    /// telling the two apart, the cursor would either stall forever behind an
    /// abandoned position, parking every entry behind it, or skip past one that
    /// had not arrived yet, dropping a message the relay had accepted.
    ///
    /// Entries are consumed as the cursor advances over them, so this holds only
    /// the gaps ahead of the cursor rather than the whole history of a target.
    pub(super) retired: BTreeSet<EntrySequence>,
    /// Units this target has resolved, and the range each covered, kept so a
    /// repeated acknowledgment is answered as the no-op it is rather than as an
    /// acknowledgment of something never declared.
    ///
    /// Bounded by the mailbox's own lifetime rather than the process's: the
    /// record belongs to this target's mailbox and goes when it does. Within one
    /// consumer generation it grows by one small entry per resolved unit, and the
    /// points that reclaim it are generation replacement and the worker-registry
    /// reap of a target torn down without replacement.
    pub(super) acknowledged: HashMap<PackingUnitId, EntryRange>,
    /// The generation currently entitled to peek, declare, and acknowledge.
    ///
    /// Every one of those operations is checked against this before it takes
    /// effect. The sequence it is drawn from — monotonic, never reused, never
    /// reset — and the fence-gated path that replaces it are relay-owned and
    /// specified by `Consumer Generation Ownership and Replacement`.
    pub(super) generation: ConsumerGenerationId,
}

impl TargetMailbox {
    /// Records that a position will never be served, and advances the cursor
    /// over any run of such positions standing at the head.
    ///
    /// The cursor advances only over positions known to be retired, never over
    /// merely-absent ones, because an admitted entry whose payload has not been
    /// enqueued yet is also absent and must still be waited for. Advancing past
    /// one would drop a message the relay had already accepted.
    pub(super) fn retire(&mut self, sequence: EntrySequence) {
        self.slots.remove(&sequence);
        // Positions already behind the cursor are not recorded: nothing reads
        // them again, and keeping them would grow the set by one entry per
        // message the target ever received.
        if sequence.value() > self.cursor.value() {
            self.retired.insert(sequence);
        }
        loop {
            let next = self.cursor.next_sequence();
            if !self.retired.remove(&next) {
                break;
            }
            self.cursor = CursorPosition::advanced_through(next);
        }
    }

    /// Releases an outstanding declaration once nothing it named is still held.
    ///
    /// A declared entry can reach a terminal state without an acknowledgment —
    /// a lifecycle trigger resolves it through the guard's evidence order. When
    /// the last of a unit's members goes that way the declaration describes
    /// nothing, and leaving it in place would refuse every later declaration for
    /// the target with a unit that can never be acknowledged.
    ///
    /// The unit is recorded as resolved rather than forgotten, so an executor
    /// that acknowledges it afterwards is told it is already terminalized, which
    /// is what happened, instead of being told it never declared it.
    pub(super) fn reconcile_outstanding(&mut self) {
        let Some(outstanding) = self.outstanding else {
            return;
        };
        if outstanding
            .range
            .sequences()
            .any(|sequence| self.slots.contains_key(&sequence))
        {
            return;
        }
        self.acknowledged
            .insert(outstanding.unit, outstanding.range);
        self.outstanding = None;
    }
}

impl Default for TargetMailbox {
    fn default() -> Self {
        Self {
            slots: BTreeMap::new(),
            cursor: CursorPosition::start(),
            next_sequence: EntrySequence::first(),
            outstanding: None,
            retired: BTreeSet::new(),
            acknowledged: HashMap::new(),
            generation: ConsumerGenerationId::new(1),
        }
    }
}

#[derive(Default)]
pub(super) struct LedgerState {
    /// Live reservations by message id. The authoritative record of what is
    /// reserved: the counters below are maintained in the same locked section, so
    /// they cannot drift from it.
    pub(super) entries: HashMap<String, AdmittedEntry>,
    /// Live packing units by id, holding the evidence their members resolve from.
    pub(super) units: HashMap<PackingUnitId, UnitRecord>,
    /// Per-target ordered mailboxes, keyed the same way usage is.
    pub(super) mailboxes: HashMap<AdmissionTargetKey, TargetMailbox>,
    pub(super) global: TargetUsage,
    pub(super) per_target: HashMap<AdmissionTargetKey, TargetUsage>,
}

#[derive(Default)]
pub(super) struct AdmissionLedger {
    state: Mutex<LedgerState>,
}

static ADMISSION_LEDGER: OnceLock<AdmissionLedger> = OnceLock::new();

fn ledger() -> &'static AdmissionLedger {
    ADMISSION_LEDGER.get_or_init(AdmissionLedger::default)
}

pub(super) fn lock_ledger() -> Result<std::sync::MutexGuard<'static, LedgerState>, RelayError> {
    ledger().state.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock the delivery admission ledger",
            None,
        )
    })
}
