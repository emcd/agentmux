//! The process-global reservation ledger and the one lock that guards it.
//!
//! Every event in this subsystem — admission, authorization, binding, evidence,
//! the terminal transition, and the reporting pass — mutates this state under
//! [`lock_ledger`], acquired exactly once at the head of the entry point and held
//! for the whole operation.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
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
    pub(super) state: QueueEntryState,
    /// Assigned at authorization and never reassigned; `None` while `Pending`.
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

#[derive(Default)]
pub(super) struct LedgerState {
    /// Live reservations by message id. The authoritative record of what is
    /// reserved: the counters below are maintained in the same locked section, so
    /// they cannot drift from it.
    pub(super) entries: HashMap<String, AdmittedEntry>,
    /// Live packing units by id, holding the evidence their members resolve from.
    pub(super) units: HashMap<PackingUnitId, UnitRecord>,
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
