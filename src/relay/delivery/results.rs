use serde_json::{Value, json};

use super::{ChatOutcome, ChatResult};

pub(super) const ACP_DELIVERY_PHASE_ACCEPTED_IN_PROGRESS: &str = "accepted_in_progress";

pub(super) fn delivered_result(target_session: String, message_id: String) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

pub(super) fn delivered_in_progress_result(
    target_session: String,
    message_id: String,
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Delivered,
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
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Failed,
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
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.into()),
        details,
    }
}

pub(super) fn timeout_result(
    target_session: String,
    message_id: String,
    reason_code: Option<&str>,
    reason: impl Into<String>,
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Timeout,
        reason_code: reason_code.map(ToString::to_string),
        reason: Some(reason.into()),
        details: None,
    }
}
