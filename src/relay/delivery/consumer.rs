//! The relay's implementation of the transport-facing mailbox seam.
//!
//! A transport's delivery-loop executor consumes its target's mailbox by calling
//! in, and it may not name a `crate::relay` type to do so. This is the handle it
//! is given: an opaque `Arc<dyn MailboxConsumer>` the relay builds closing over
//! the target and the generation the executor is entitled to consume under —
//! the same injected-closure shape the chooser, the readiness notifier and the
//! doorbell already use, for the one call direction that runs the other way.
//!
//! **The binding is held here rather than passed per call.** The requirement
//! names the operations `peek(target, ...)`, `declare(target, generation_id,
//! ...)` and `ack(target, generation_id, ...)`, and the check is unchanged: the
//! ledger still compares the supplied generation against the target's active one
//! as the first thing it does under its lock. What moves is only where the
//! binding is kept. An executor that cannot name a target cannot name the wrong
//! one, and a generation identifier that never reaches the transport cannot be
//! retained past the replacement that superseded it.
//!
//! # Reporting is this layer's, not the ledger's
//!
//! Three of these operations resolve entries, and none of them may report from
//! inside the ledger lock — reporting emits inscriptions, routes a receipt
//! through another target's worker, and takes locks of its own. So the ledger
//! resolves under its lock and hands back what it resolved, and this publishes
//! it once the lock is gone. That split is why an acknowledgment reaches a
//! sender at all: the executor that made it knows sequence numbers, and the send
//! owed an outcome is known only to the mailbox that held the entry.

use std::sync::Arc;

use crate::protocol::PackingUnitId;
use crate::protocol::identity::ConsumerBinding;
use crate::protocol::mailbox::EntryRange;
use crate::protocol::operations::{
    AckResult, DeclareResult, MemberAcknowledgment, PeekRequest, PeekResult,
};
use crate::transports::MailboxConsumer;

use super::admission::{Acknowledgment, ResolvedMember};
use super::async_worker::report_resolved_member;

/// The reason code a member resolved by a sustained unreachability carries.
///
/// Kept identical to the code the push model's dwell reported, because it is the
/// same finding about the same target reaching the same sender: what changed is
/// which layer observed it, and a sender correlating receipts across the cutover
/// should see no difference.
const UNREACHABLE_REASON_CODE: &str = "delivery_target_unreachable";
const UNREACHABLE_REASON: &str = "target could not be reached for longer than the configured dwell";

/// One target's mailbox, bound to the generation entitled to consume it.
pub(in crate::relay) struct LedgerMailboxConsumer {
    binding: ConsumerBinding,
}

impl LedgerMailboxConsumer {
    /// Builds the handle a generation's delivery-loop executor consumes through.
    ///
    /// Takes the binding rather than issuing it, because issuing a generation is
    /// a lifecycle act — a claim or a fenced replacement — and doing it here
    /// would mean building a handle was enough to take a target from whoever
    /// held it. Named for what it does rather than spelled `new`, since what
    /// comes back is the trait object the transport side holds and never this
    /// type.
    pub(in crate::relay) fn bind(binding: ConsumerBinding) -> Arc<dyn MailboxConsumer> {
        Arc::new(Self { binding })
    }

    /// Publishes what a resolving operation resolved.
    ///
    /// Runs after the ledger lock is released, on every path. Each member here
    /// won its terminal transition inside that lock, so this is the one report
    /// it will ever get — a caller that skipped it would leave a sender waiting
    /// on a receipt for a message that had already resolved.
    fn report(resolved: &[ResolvedMember], reason_code: Option<&str>, reason: Option<&str>) {
        for member in resolved {
            report_resolved_member(member, reason_code, reason);
        }
    }
}

impl MailboxConsumer for LedgerMailboxConsumer {
    fn peek(&self, entry_max: usize, canonical_bytes_max: u64) -> PeekResult {
        super::admission::peek(&PeekRequest {
            binding: self.binding.clone(),
            entry_max,
            canonical_bytes_max,
        })
    }

    fn declare(&self, range: EntryRange) -> DeclareResult {
        super::admission::declare(&self.binding, range)
    }

    fn ack(&self, unit: PackingUnitId, members: &[MemberAcknowledgment]) -> AckResult {
        let Acknowledgment { result, resolved } =
            super::admission::ack(&self.binding, unit, members);
        // No cause: the executor reported what its write observed for each
        // member, and the evidence is the whole story. Naming a reason here
        // would be this layer narrating a write it did not perform.
        Self::report(&resolved, None, None);
        result
    }

    fn resolve_unreachable(&self) {
        let resolved = super::admission::resolve_target_entries(&self.binding);
        Self::report(
            &resolved,
            Some(UNREACHABLE_REASON_CODE),
            Some(UNREACHABLE_REASON),
        );
    }
}
