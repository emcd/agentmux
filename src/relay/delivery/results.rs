use serde_json::{Value, json};

use super::super::{SendOutcome, SendResult};

pub(super) const ACP_DELIVERY_PHASE_ACCEPTED_IN_PROGRESS: &str = "accepted_in_progress";

pub(super) fn delivered_result(target_session: String, message_id: String) -> SendResult {
    SendResult {
        target_session,
        message_id,
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

pub(super) fn delivered_in_progress_result(
    target_session: String,
    message_id: String,
) -> SendResult {
    SendResult {
        target_session,
        message_id,
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: Some(json!({
            "delivery_phase": ACP_DELIVERY_PHASE_ACCEPTED_IN_PROGRESS,
        })),
    }
}

pub(super) fn failed_result(
    target_session: String,
    message_id: String,
    reason: impl Into<String>,
) -> SendResult {
    SendResult {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: None,
        reason: Some(reason.into()),
        details: None,
    }
}

pub(super) fn failed_result_with_code(
    target_session: String,
    message_id: String,
    reason_code: &str,
    reason: impl Into<String>,
    details: Option<Value>,
) -> SendResult {
    SendResult {
        target_session,
        message_id,
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.into()),
        details,
    }
}

// TODO(refactor-acp-background-reader follow-up): no callers after task 5.
// Drop together with the rest of the relay-side timeout vestiges.
#[allow(dead_code)]
pub(super) fn timeout_result(
    target_session: String,
    message_id: String,
    reason_code: Option<&str>,
    reason: impl Into<String>,
) -> SendResult {
    SendResult {
        target_session,
        message_id,
        outcome: SendOutcome::Timeout,
        reason_code: reason_code.map(ToString::to_string),
        reason: Some(reason.into()),
        details: None,
    }
}
