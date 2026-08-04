//! Outcome shaping and timestamp helpers for the async-delivery worker.
//!
//! Five free fns extracted from `worker.rs`:
//! - `now_rfc3339` formats the current UTC time as RFC 3339. Used by the
//!   envelope builders to stamp `created_at` on every emitted `RelayStreamEvent`.
//! - `acp_respawn_stream_event` builds the canonical `RelayStreamEvent` template
//!   the ACP worker broadcasts to watching hosts on respawn.
//! - `outcome_to_send_result`/`dropped_send_result` map a transport-side
//!   `SingleDeliveryOutcome` (or its absence) onto a relay-side `SendResult`,
//!   filling in the relay-authoritative `target_session` and `message_id`.
//! - `collect_outcome` is the join-handler at the producer end of the
//!   producer-and-collect loop: it routes a resolved outcome onto the relay's
//!   bookkeeping (record-served, complete-task, release-slot), treats a drop
//!   as a shutdown, and treats a panicked collector task as a release-only.
//!
//! (`stream_send_to_broadcast_status` lives in `envelope.rs` because every
//! call site is inside an envelope builder.)

use std::collections::HashMap;

use super::super::async_worker;
use super::super::guard::{GuardTrigger, SubmissionEvidence};
use super::worker::InflightMember;
use crate::relay::{
    AsyncDeliveryTask, SendResult, identity::canonical_session_id,
    startup_state::note_session_served_successfully, stream::RelayStreamEvent,
};
use crate::transports::{SendOutcome, SingleDeliveryOutcome};
use time::format_description::well_known::Rfc3339;

pub(super) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Builds the canonical `RelayStreamEvent` template the ACP worker broadcasts
/// to watching hosts on respawn. The `target_session` field is rewritten per
/// recipient by the broadcast_ui closure in `build_acp_driver_services`.
pub(super) fn acp_respawn_stream_event(
    event_type: &str,
    namespace: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        target_session: canonical_session_id(target_session, namespace),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload,
    }
}

/// Maps a transport outcome onto the relay `SendResult`, substituting the task's
/// own correlation fields (the transport leaves them blank; the relay is
/// authoritative for them). Shared by every transport — the worker dispatches
/// `mailw`/`raww` uniformly, so the collect site maps outcomes uniformly too.
fn outcome_to_send_result(task: &AsyncDeliveryTask, outcome: SingleDeliveryOutcome) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: outcome.outcome,
        reason_code: outcome.reason_code,
        reason: outcome.reason,
        details: outcome.details,
    }
}

/// The evidence a resolved transport outcome establishes.
///
/// `Delivered` is the only spelling that positively reports a target-side
/// effect. Everything else is an undifferentiated failure from the guard's point
/// of view, and an undifferentiated failure maps to `SubmissionUnknown` — never
/// `NotSubmitted`, which requires proof that nothing was written.
fn evidence_from_outcome(outcome: &SingleDeliveryOutcome) -> SubmissionEvidence {
    match outcome.outcome {
        SendOutcome::Delivered => SubmissionEvidence::Submitted,
        SendOutcome::NotSubmitted => SubmissionEvidence::NotSubmitted,
        _ => SubmissionEvidence::SubmissionUnknown,
    }
}

/// Resolves one joined collector task, whether it completed or panicked.
///
/// Both paths look the member up in the worker's table rather than taking it
/// from the collector, so a panicked collector no longer strands its member: the
/// guard resolves it through the same evidence order every other lifecycle
/// trigger uses. This is what makes exactly-once resolution a property of the
/// system rather than an assumption about well-behaved tasks.
pub(super) fn collect_outcome(
    joined: Result<(tokio::task::Id, Option<SingleDeliveryOutcome>), tokio::task::JoinError>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    let (task_id, outcome, trigger) = match joined {
        Ok((task_id, Some(outcome))) => (task_id, Some(outcome), None),
        // The future was dropped before resolving: the transport's delivery task
        // vanished with the write in its hands.
        Ok((task_id, None)) => (task_id, None, Some(GuardTrigger::ChannelClosed)),
        Err(join_error) => (join_error.id(), None, Some(GuardTrigger::CollectorPanic)),
    };
    let Some(InflightMember {
        task,
        record_served,
    }) = inflight_members.remove(&task_id)
    else {
        // No member for this collector: it was already removed, so nothing is
        // owed. Releasing the slot is still correct — it was reserved per write.
        async_worker::release_pending_slot(pending);
        return;
    };

    match (outcome, trigger) {
        (Some(outcome), _) => {
            // The unit's evidence is recorded before any member outcome is
            // derived from it, so a resumed fan-out agrees with what the first
            // pass reported instead of inventing a result for the remainder.
            super::super::admission::record_unit_evidence(
                task.message_id.as_str(),
                evidence_from_outcome(&outcome),
            );
            let send_result = outcome_to_send_result(&task, outcome);
            if record_served && send_result.outcome == SendOutcome::Delivered {
                let _ = note_session_served_successfully(
                    task.runtime_directory.as_path(),
                    task.target_session.as_str(),
                );
            }
            async_worker::complete_task_outcome(&task, Ok(send_result));
        }
        (None, Some(trigger)) => {
            async_worker::complete_task_outcome_from_trigger(&task, trigger);
        }
        (None, None) => unreachable!("a resolved outcome or a trigger is always present"),
    }
    async_worker::release_pending_slot(pending);
}
