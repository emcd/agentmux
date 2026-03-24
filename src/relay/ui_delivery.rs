use std::{
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::configuration::BundleMember;
use crate::runtime::signals::shutdown_requested;

use super::stream::{RelayStreamEvent, StreamEventSendOutcome, send_event_to_registered_ui};
use super::{AsyncDeliveryTask, ChatOutcome, ChatResult};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const UI_RECONNECT_POLL_INTERVAL_MS: u64 = 100;

pub(super) fn deliver_one_target_ui(
    task: &AsyncDeliveryTask,
    sender: &BundleMember,
    cc_members: &[BundleMember],
    target_session: String,
    message_id: String,
    message: &str,
) -> ChatResult {
    let bundle_name = task.bundle.bundle_name.as_str();
    let timeout = task.quiescence.quiescence_timeout;
    let start = Instant::now();
    loop {
        if shutdown_requested() {
            return ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            };
        }

        let incoming_event = RelayStreamEvent {
            event_type: "incoming_message".to_string(),
            bundle_name: bundle_name.to_string(),
            target_session: target_session.clone(),
            created_at: timestamp_rfc3339(),
            payload: json!({
                "message_id": message_id.clone(),
                "sender_session": sender.id.as_str(),
                "body": message,
                "cc_sessions": if cc_members.is_empty() {
                    Value::Null
                } else {
                    json!(cc_members.iter().map(|member| member.id.clone()).collect::<Vec<_>>())
                },
            }),
        };
        match send_event_to_registered_ui(bundle_name, target_session.as_str(), &incoming_event) {
            Ok(StreamEventSendOutcome::Delivered) => {
                let outcome_event = RelayStreamEvent {
                    event_type: "delivery_outcome".to_string(),
                    bundle_name: bundle_name.to_string(),
                    target_session: target_session.clone(),
                    created_at: timestamp_rfc3339(),
                    payload: json!({
                        "message_id": message_id.clone(),
                        "outcome": "success",
                    }),
                };
                let _ = send_event_to_registered_ui(
                    bundle_name,
                    target_session.as_str(),
                    &outcome_event,
                );
                return ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Delivered,
                    reason_code: None,
                    reason: None,
                    details: None,
                };
            }
            Ok(StreamEventSendOutcome::NoUiEndpoint) | Ok(StreamEventSendOutcome::Disconnected) => {
            }
            Err(source) => {
                return ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Failed,
                    reason_code: None,
                    reason: Some(format!("failed to emit relay stream event: {}", source)),
                    details: None,
                };
            }
        }
        if timeout.is_some_and(|value| start.elapsed() >= value) {
            return ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::Timeout,
                reason_code: None,
                reason: Some(format!(
                    "ui relay stream was disconnected for {}ms",
                    start.elapsed().as_millis()
                )),
                details: None,
            };
        }
        thread::sleep(Duration::from_millis(UI_RECONNECT_POLL_INTERVAL_MS));
    }
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
