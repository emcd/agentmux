use std::{
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::runtime::signals::shutdown_requested;

use super::super::canonical_session_id;
use super::super::stream::{RelayStreamEvent, StreamEventSendOutcome, send_event_to_registered_ui};
use super::super::{AsyncDeliveryTask, SendOutcome, SendResult};
use super::quiescence::QUIESCENCE_TIMEOUT_MS_DEFAULT;

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const UI_RECONNECT_POLL_INTERVAL_MS: u64 = 100;

pub(super) fn deliver_one_target_ui(
    task: &AsyncDeliveryTask,
    sender_session: &str,
    cc_sessions: &[String],
    target_session: String,
    message_id: String,
    message: &str,
) -> SendResult {
    let bundle_name = task.bundle.bundle_name.as_str();
    // The async send path leaves `quiescence_timeout` unset by default; cap the
    // reconnect wait with the sync-path default so a permanently absent UI
    // cannot pin this delivery worker's blocking thread forever
    // (issues/relay/26 defense-in-depth).
    let timeout = task
        .quiescence
        .quiescence_timeout
        .unwrap_or(Duration::from_millis(QUIESCENCE_TIMEOUT_MS_DEFAULT));
    let start = Instant::now();
    loop {
        if shutdown_requested() {
            let _ = emit_delivery_outcome_event(
                bundle_name,
                target_session.as_str(),
                message_id.as_str(),
                "failed",
                Some("failed"),
                Some(DROPPED_ON_SHUTDOWN_REASON_CODE),
                Some(DROPPED_ON_SHUTDOWN_REASON),
            );
            return SendResult {
                target_session,
                message_id,
                outcome: SendOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            };
        }

        let mut payload = json!({
            "message_id": message_id.clone(),
            // The sender is attributed to its home bundle, which differs
            // from `bundle_name` (the target's bundle) for cross-bundle sends.
            "sender_session": canonical_session_id(
                sender_session,
                task.sender_bundle_name.as_str(),
            ),
            "body": message,
            "cc_sessions": if cc_sessions.is_empty() {
                Value::Null
            } else {
                json!(
                    cc_sessions
                        .iter()
                        .map(|cc| canonical_session_id(cc, bundle_name))
                        .collect::<Vec<_>>()
                )
            },
        });
        // Carry the sender's verified principal_id to the recipient so it can
        // build authorization on top. Omitted (not null) for socket-trust
        // senders, mirroring the Send response.
        if let Some(authenticated_identity) = task.authenticated_identity.as_deref() {
            payload["authenticated_identity"] = Value::String(authenticated_identity.to_string());
        }
        let incoming_event = RelayStreamEvent {
            event_type: "incoming_message".to_string(),
            bundle_name: bundle_name.to_string(),
            target_session: target_session.clone(),
            created_at: timestamp_rfc3339(),
            payload,
        };
        let routed_outcome = emit_delivery_outcome_event(
            bundle_name,
            target_session.as_str(),
            message_id.as_str(),
            "routed",
            None,
            None,
            None,
        );
        match routed_outcome {
            Ok(StreamEventSendOutcome::Delivered) => {}
            Ok(StreamEventSendOutcome::NoUiEndpoint) | Ok(StreamEventSendOutcome::Disconnected) => {
                if start.elapsed() >= timeout {
                    return SendResult {
                        target_session,
                        message_id,
                        outcome: SendOutcome::Timeout,
                        reason_code: None,
                        reason: Some(format!(
                            "ui relay stream was disconnected for {}ms",
                            start.elapsed().as_millis()
                        )),
                        details: None,
                    };
                }
                thread::sleep(Duration::from_millis(UI_RECONNECT_POLL_INTERVAL_MS));
                continue;
            }
            Err(source) => {
                return SendResult {
                    target_session,
                    message_id,
                    outcome: SendOutcome::Failed,
                    reason_code: None,
                    reason: Some(format!("failed to emit relay stream event: {}", source)),
                    details: None,
                };
            }
        }
        match send_event_to_registered_ui(bundle_name, target_session.as_str(), &incoming_event) {
            Ok(StreamEventSendOutcome::Delivered) => {
                let _ = emit_delivery_outcome_event(
                    bundle_name,
                    target_session.as_str(),
                    message_id.as_str(),
                    "delivered",
                    Some("success"),
                    None,
                    None,
                );
                return SendResult {
                    target_session,
                    message_id,
                    outcome: SendOutcome::Delivered,
                    reason_code: None,
                    reason: None,
                    details: None,
                };
            }
            Ok(StreamEventSendOutcome::NoUiEndpoint) | Ok(StreamEventSendOutcome::Disconnected) => {
            }
            Err(source) => {
                return SendResult {
                    target_session,
                    message_id,
                    outcome: SendOutcome::Failed,
                    reason_code: None,
                    reason: Some(format!("failed to emit relay stream event: {}", source)),
                    details: None,
                };
            }
        }
        if start.elapsed() >= timeout {
            return SendResult {
                target_session,
                message_id,
                outcome: SendOutcome::Timeout,
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

fn emit_delivery_outcome_event(
    bundle_name: &str,
    target_session: &str,
    message_id: &str,
    phase: &str,
    outcome: Option<&str>,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> Result<StreamEventSendOutcome, std::io::Error> {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "message_id".to_string(),
        Value::String(message_id.to_string()),
    );
    payload.insert("phase".to_string(), Value::String(phase.to_string()));
    payload.insert(
        "outcome".to_string(),
        outcome
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    if let Some(value) = reason_code {
        payload.insert("reason_code".to_string(), Value::String(value.to_string()));
    }
    if let Some(value) = reason {
        payload.insert("reason".to_string(), Value::String(value.to_string()));
    }
    let event = RelayStreamEvent {
        event_type: "delivery_outcome".to_string(),
        bundle_name: bundle_name.to_string(),
        target_session: target_session.to_string(),
        created_at: timestamp_rfc3339(),
        payload: Value::Object(payload),
    };
    send_event_to_registered_ui(bundle_name, target_session, &event)
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
