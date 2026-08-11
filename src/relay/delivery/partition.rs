//! The relay's implementation of the transport-facing partition sink.
//!
//! A transport decides how many target writes a group of envelopes becomes, and
//! that partition decides which members share a fate. The relay cannot see it,
//! so it hands each transport this handle and learns the partition from the
//! layer that chose it.
//!
//! There is nothing to configure and nothing to hold: both calls delegate
//! straight to the admission ledger, which is a process-global holding the one
//! lock under which binding and evidence are written. The type exists to be a
//! `dyn` object a transport can hold without naming `crate::relay`.

use std::sync::Arc;

use crate::transports::{PackingUnitId, PartitionError, PartitionSink, SubmissionEvidence};

use super::admission::{declare_packing_unit, record_unit_evidence};

/// A [`PartitionSink`] backed by the relay's admission ledger.
pub(in crate::relay) struct LedgerPartitionSink;

impl PartitionSink for LedgerPartitionSink {
    fn declare(&self, member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
        declare_packing_unit(member_ids)
    }

    fn record(&self, unit: PackingUnitId, evidence: SubmissionEvidence) {
        record_unit_evidence(unit, evidence);
    }
}

/// The handle handed to every transport at construction.
///
/// Required rather than optional. A transport built without a sink would have no
/// way to report its partition, and the guard would fall back to treating each
/// member as its own unit — which is not a degraded answer but a wrong one: a
/// coalesced group whose single write failed would report one member's evidence
/// and infer the rest, exactly the split outcome the unit-owned record prevents.
pub(in crate::relay) fn ledger_partition_sink() -> Arc<dyn PartitionSink> {
    Arc::new(LedgerPartitionSink)
}
