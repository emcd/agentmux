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

use super::super::async_worker;
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

/// Result for a task whose outcome future was dropped before resolving (the
/// transport's delivery task vanished); treated as a shutdown drop.
fn dropped_send_result(task: &AsyncDeliveryTask) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: SendOutcome::DroppedOnShutdown,
        reason_code: Some("dropped_on_shutdown".to_string()),
        reason: Some("delivery worker dropped before completion".to_string()),
        details: None,
    }
}

/// Maps one resolved in-flight outcome onto a `SendResult`, fans it back to the
/// originating sender, records `served_successfully` for delivered coder writes,
/// and releases the pending slot. A panicked collector task only releases the
/// slot (a panic is a bug, not a delivery result).
pub(super) fn collect_outcome(
    joined: Result<
        (AsyncDeliveryTask, bool, Option<SingleDeliveryOutcome>),
        tokio::task::JoinError,
    >,
    pending: &std::sync::atomic::AtomicUsize,
) {
    let (task, record_served, outcome) = match joined {
        Ok(value) => value,
        Err(_join_error) => {
            async_worker::release_pending_slot(pending);
            return;
        }
    };
    let send_result = match outcome {
        Some(outcome) => outcome_to_send_result(&task, outcome),
        None => dropped_send_result(&task),
    };
    if record_served && send_result.outcome == SendOutcome::Delivered {
        let _ = note_session_served_successfully(
            task.runtime_directory.as_path(),
            task.target_session.as_str(),
        );
    }
    async_worker::complete_task_outcome(&task, Ok(send_result));
    async_worker::release_pending_slot(pending);
}
