//! Taking custody of a task: building the artifact its target will be written,
//! and placing that artifact in the target's mailbox.
//!
//! This is the delivery worker's task intake — the first point the target's own
//! worker holds a task, and still the last one before any transport is consulted.
//! The payload is built here rather than at the write, and built exactly once:
//! [`submit`](super::submit) sends *this* artifact rather than rendering a second
//! one, so what the mailbox holds and what reaches the target cannot differ.
//!
//! Nothing peeks yet. The push path is still the only delivery path, and this
//! seam neither adds one nor removes one — it moves where the artifact is built,
//! which is what puts a delivery-loop executor's future input under the
//! production suite while the push path is still there to deliver it.

use std::sync::Arc;

use crate::protocol::mailbox::MailboxPayload;
use crate::relay::{AsyncDeliveryTask, DeliveryPayloadMode, RelayError};
use crate::transports::TransportImpl;

use super::super::super::admission::enqueue;
use super::super::envelope::{build_coder_envelope, build_ui_envelope};
use super::super::outcomes::now_rfc3339;
use super::super::payload::{
    build_delivery_message, emit_envelope_metadata_inscription, resolve_target_member,
};

/// A task the worker has taken custody of, with the artifact its target is to be
/// written.
pub(super) struct IntakeTask {
    /// Shared with the mailbox, which holds the same handle for as long as the
    /// entry occupies a position: an acknowledgment names a sequence number, and
    /// only the task names the sender that acknowledgment owes an outcome.
    pub(super) task: Arc<AsyncDeliveryTask>,
    /// What this entry's mailbox slot holds, or the refusal that stopped one from
    /// being built.
    ///
    /// The refusal is carried rather than reported at intake because the target
    /// gate has not run yet: reporting here would resolve a member for a target
    /// the relay had not consulted, and a held member would be resolved while it
    /// is still owed a delivery. It is reported at the same point it was before
    /// this seam existed, immediately before the write it stands in for.
    pub(super) payload: Result<MailboxPayload, RelayError>,
}

/// Builds one task's payload, stamps it once, and enqueues it into its target's
/// mailbox.
pub(super) fn take_into_mailbox(task: AsyncDeliveryTask, transport: &TransportImpl) -> IntakeTask {
    let task = Arc::new(task);
    // One read of the clock per entry. `Date` therefore names when the relay
    // built the envelope rather than when a transport got round to writing it,
    // which is the only stamp that can be the same on both sides of the mailbox.
    let payload = build_payload(&task, transport, now_rfc3339().as_str());
    if let Ok(payload) = payload.as_ref() {
        // Delivery does not depend on this succeeding, and must not. A
        // terminal-outcome receipt bypasses admission, so it holds no ledger
        // entry and no position and is refused `NotAdmitted` here — it is still
        // owed to its sender, and it reaches them through the push path all the
        // same. The same holds for any entry whose position the ledger refuses:
        // a refused enqueue leaves the mailbox without the entry, never the
        // sender without an answer.
        let _ = enqueue(&task, payload.clone());
    }
    IntakeTask { task, payload }
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
