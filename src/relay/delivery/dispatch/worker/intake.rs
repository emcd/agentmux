//! Taking custody of a task: building the artifact its target will be written,
//! and placing that artifact in the target's mailbox.
//!
//! This is the delivery worker's task intake — the first point the target's own
//! worker holds a task, and the last point it is involved in the delivery at all.
//! The payload is built here and built exactly once, so what the mailbox holds
//! and what reaches the target cannot differ: the executor writes the stored
//! artifact rather than rendering a second one.
//!
//! **Everything that can go wrong here is reported here**, because there is no
//! later step to report it at. Under the push model a refusal was carried
//! forward to the write it stood in for, so the target gate could run first;
//! there is no such write and no such gate any more, and an entry that never
//! reaches the mailbox is one no executor will ever see.

use std::sync::Arc;

use crate::protocol::mailbox::MailboxPayload;
use crate::relay::{AsyncDeliveryTask, DeliveryPayloadMode, RelayError};
use crate::transports::TransportImpl;

use super::super::super::admission::{EnqueueRejection, enqueue};
use super::super::super::async_worker::{complete_task_refusal, release_pending_slot};
use super::super::envelope::{build_coder_envelope, build_ui_envelope};
use super::super::outcomes::now_rfc3339;
use super::super::payload::{
    build_delivery_message, emit_envelope_metadata_inscription, resolve_target_member,
};

/// Builds one task's payload, stamps it once, and enqueues it into its target's
/// mailbox.
///
/// The pending slot travels with the entry: it is released at the terminal
/// transition, which for an enqueued entry is its acknowledgment. It is released
/// here only on the two paths where no entry exists to release it later.
pub(super) fn take_into_mailbox(
    task: AsyncDeliveryTask,
    transport: &TransportImpl,
    pending: &std::sync::atomic::AtomicUsize,
) {
    let task = Arc::new(task);
    // One read of the clock per entry. `Date` therefore names when the relay
    // built the envelope rather than when a transport got round to writing it,
    // which is the only stamp that can be the same on both sides of the mailbox.
    let payload = match build_payload(&task, transport, now_rfc3339().as_str()) {
        Ok(payload) => payload,
        Err(error) => {
            // No payload means no entry, and no entry means nothing will ever
            // write this. Routed through the guard rather than reported as an
            // explicit error: an `Err` would spell it `failed`, the
            // undifferentiated outcome, for a member the evidence order can prove
            // was never submitted. The refusal's own code and message travel with
            // it, because the guard knows the member was not written but not that
            // its target member could not be resolved.
            complete_task_refusal(&task, error.code.as_str(), error.message.as_str());
            release_pending_slot(pending);
            return;
        }
    };
    if let Err(rejection) = enqueue(&task, payload) {
        // The ledger refused the position. Every shape of this is a relay-side
        // condition rather than anything about the target, and each leaves the
        // entry unwritten — so the sender is told so here rather than waiting on
        // an executor that will never see it.
        let (code, reason) = match rejection {
            EnqueueRejection::AlreadyEnqueued => (
                "internal_unexpected_failure",
                "this message already occupies its position in the target's mailbox",
            ),
            EnqueueRejection::AlreadyTerminal => (
                "internal_unexpected_failure",
                "this message had already reached a terminal outcome",
            ),
            EnqueueRejection::LedgerUnavailable => (
                "internal_unexpected_failure",
                "the relay could not reach its delivery ledger to queue this message",
            ),
        };
        complete_task_refusal(&task, code, reason);
        release_pending_slot(pending);
    }
}

/// Builds the artifact a target is to be written, from the task alone.
///
/// Touches no transport beyond reading which kind it is, so it can run at intake
/// — before the gate, before readiness, before anything that could produce a
/// target-side effect.
fn build_payload(
    task: &AsyncDeliveryTask,
    transport: &TransportImpl,
    created_at: &str,
) -> Result<MailboxPayload, RelayError> {
    // A UI target is written an envelope whatever the task's payload mode: it is
    // served by a stream event, which has no raw form. This is the dispatch the
    // write path made before the payload moved here, kept rather than widened —
    // a raw payload for a UI target would name a write no UI transport performs.
    if matches!(transport, TransportImpl::Ui(_)) {
        return Ok(MailboxPayload::Mail(Arc::new(build_ui_envelope(
            task, created_at,
        ))));
    }
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let target_member = resolve_target_member(task)?;
            let message = build_delivery_message(task, target_member, created_at);
            // Emitted where the payload is built, and only here. Left at the write
            // it would describe an envelope other than the one the mailbox holds;
            // emitted at both points it would double-count. Raw input carries no
            // envelope and so emits none, exactly as before.
            emit_envelope_metadata_inscription(&message, task.message_id.as_str());
            Ok(MailboxPayload::Mail(Arc::new(build_coder_envelope(
                task, message,
            ))))
        }
        DeliveryPayloadMode::RawInput => Ok(MailboxPayload::Raw {
            content: task.message.clone(),
            append_enter: task.append_enter,
        }),
    }
}
