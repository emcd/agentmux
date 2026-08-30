//! Publishing a resolved member's terminal outcome: the observability floor, the
//! sender's `delivery_outcome` event, and the receipt routed back to whoever sent
//! the message.
//!
//! Everything here runs *after* the terminal transition has been won, so each
//! emission happens exactly once per member. Nothing here may fail the member: a
//! notification that cannot be delivered is counted and recorded, never
//! propagated, because the member is already resolved and refusing to release its
//! quota over a reporting problem would leak the reservation.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::configuration::{BundleConfiguration, BundleMember, TargetConfiguration};
use crate::runtime::inscriptions::emit_inscription;

use crate::relay::delivery::guard::GuardKey;
use crate::relay::stream::{RelayStreamEvent, StreamEventSendOutcome, send_event_to_registered_ui};
use crate::relay::{
    AsyncDeliveryTask, DeliveryPayloadMode, RELAY_NAMESPACE, RelayError, SCHEMA_VERSION,
    SendOutcome, SendResult, SenderReturnRoute, canonical_session_id,
};

use super::registry::{WorkerDispatch, build_worker_key, try_existing_worker};

/// Emits the observability floor for a resolved member and routes its receipt.
///
/// Reached only after the terminal transition has been won, so everything it
/// emits happens exactly once per member.
pub(super) fn report_terminal_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<SendResult, RelayError>,
    guard: Option<GuardKey>,
) {
    // The terminal outcome (and its reason), independent of the Ok/Err shape, so a
    // non-delivered outcome can be routed back to the sender as a receipt after the
    // observability floor is emitted below. A relay-side delivery error resolves to
    // `Failed`, matching the inscription arm.
    let (terminal_outcome, terminal_reason_code, terminal_reason) = match &outcome {
        Ok(result) => (
            result.outcome.clone(),
            result.reason_code.clone(),
            result.reason.clone(),
        ),
        Err(error) => (
            SendOutcome::Failed,
            Some(error.code.clone()),
            Some(error.message.clone()),
        ),
    };
    match outcome {
        Ok(result) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_namespace.as_str(),
                task.sender.id.as_str(),
                result.target_session.as_str(),
                result.message_id.as_str(),
                result.outcome.clone(),
                result.reason_code.as_deref(),
                result.reason.as_deref(),
            );
            emit_inscription(
                "relay.send.async.completed",
                &json!({
                    "namespace": task.bundle.bundle_name,
                    "sender_session": task.sender.id,
                    "target_session": result.target_session,
                    "message_id": result.message_id,
                    "outcome": result.outcome,
                    "reason_code": result.reason_code,
                    "reason": result.reason,
                    "details": result.details,
                    "entry_sequence": guard.map(|key| key.sequence().value()),
                }),
            );
        }
        Err(error) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_namespace.as_str(),
                task.sender.id.as_str(),
                task.target_session.as_str(),
                task.message_id.as_str(),
                SendOutcome::Failed,
                Some(error.code.as_str()),
                Some(error.message.as_str()),
            );
            emit_inscription(
                "relay.send.async.completed",
                &json!({
                    "namespace": task.bundle.bundle_name,
                    "sender_session": task.sender.id,
                    "target_session": task.target_session,
                    "message_id": task.message_id,
                    "outcome": SendOutcome::Failed,
                    "reason": error.message,
                    "error_code": error.code,
                }),
            );
        }
    }
    // With the observability floor recorded above for every terminal outcome,
    // best-effort a terminal-outcome receipt back to the sender for the
    // non-delivered ones.
    deliver_terminal_outcome_receipt(
        task,
        &terminal_outcome,
        terminal_reason_code.as_deref(),
        terminal_reason.as_deref(),
    );
}

/// The relay/system principal that a terminal-outcome receipt is attributed to,
/// rendered as `relay@RELAY` in the receipt envelope so a recipient can tell it
/// from inbound peer traffic.
const TERMINAL_RECEIPT_SENDER_ID: &str = "relay";

/// Whether a terminal outcome is *non-delivered* and therefore warrants a
/// receipt. `Delivered` is success (recorded on the floor only), `Queued` is not
/// terminal, and `PeerUnavailable` is a cross-relay outcome reported
/// synchronously on the send response — none produce a receipt.
fn is_non_delivered_outcome(outcome: &SendOutcome) -> bool {
    matches!(
        outcome,
        SendOutcome::Failed
            | SendOutcome::DroppedOnShutdown
            // Both are terminal non-delivery outcomes and owe the sender a
            // receipt exactly as much as the others do. `not_submitted` is a
            // sound assertion that nothing arrived; `submission_unknown` is the
            // absence of one, which a sender arguably needs to hear about more
            // urgently than a plain failure, not less.
            | SendOutcome::NotSubmitted
            | SendOutcome::SubmissionUnknown
    )
}

/// Best-effort delivers a terminal-outcome receipt back to the original sender
/// when a queued message resolves to a non-delivered terminal outcome.
///
/// This is the single non-recursion chokepoint: a receipt is spawned only when
/// the resolving delivery is itself not a receipt (`is_receipt == false`) and its
/// outcome is non-delivered. The receipt routes ONLY to the sender's already-live
/// delivery worker via [`try_existing_worker`], which bounces the task back
/// rather than spawning a worker when the sender is not routable — so an
/// unreachable sender's receipt is dropped, never persisted or retried. The
/// terminal outcome is already on the `relay.log` floor regardless.
fn deliver_terminal_outcome_receipt(
    task: &AsyncDeliveryTask,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) {
    if task.is_receipt || !is_non_delivered_outcome(outcome) {
        return;
    }
    // A sender with no home bundle (`GLOBAL`/`RELAY`) carries no return route; a
    // UI operator among them is served by the `delivery_outcome` stream frame the
    // observability path already emitted.
    let Some(route) = task.sender_return_route.as_ref() else {
        return;
    };
    let receipt = build_terminal_outcome_receipt(task, route, outcome, reason_code, reason);
    let sender_key = build_worker_key(
        task.sender_namespace.as_str(),
        route.runtime_directory.as_path(),
        route.member.id.as_str(),
    );
    // Route to the sender's live worker only; drop on any non-delivery (no live
    // worker, or a bounded ACP queue that is full). Never spawn a worker for a
    // receipt — that is the deliberate boundary against deferred delivery.
    //
    // An absent worker is an ordinary state and is not counted: the sender has
    // no delivery path at all, the same class of condition as a sender with no
    // attached UI, and counting it would fire constantly for offline senders.
    //
    // Everything else is a path that existed and did not carry the outcome. A
    // dropped receiver is the important one: the sender *was* reachable, so this
    // is a notification path that closed rather than one that was never there,
    // and the contract requires a closed or dropped path be counted and
    // recorded. A draining worker and an unreadable registry are the relay's own
    // problems and count for the same reason.
    match try_existing_worker(&sender_key, receipt) {
        Ok(WorkerDispatch::Accepted) | Ok(WorkerDispatch::Missing(_)) => {}
        Ok(WorkerDispatch::Dropped(_)) => {
            note_outcome_notification_failure(
                "receipt",
                task.message_id.as_str(),
                "sender_worker_receiver_dropped",
            );
        }
        Ok(WorkerDispatch::Closing(_)) => {
            note_outcome_notification_failure(
                "receipt",
                task.message_id.as_str(),
                "sender_worker_closing",
            );
        }
        Ok(WorkerDispatch::FailStopped(_)) => {
            note_outcome_notification_failure(
                "receipt",
                task.message_id.as_str(),
                "sender_worker_fail_stopped",
            );
        }
        Err(error) => {
            note_outcome_notification_failure(
                "receipt",
                task.message_id.as_str(),
                error.code.as_str(),
            );
        }
    }
}

/// Builds the receipt delivery task addressed back to the original sender. The
/// receipt carries a minimal delivery bundle holding just the sender's real
/// member, so the delivery worker resolves the sender's true transport and
/// renders/delivers the receipt through it. It is attributed to the relay/system
/// principal and marked `is_receipt` so its own terminal outcome spawns nothing.
fn build_terminal_outcome_receipt(
    task: &AsyncDeliveryTask,
    route: &SenderReturnRoute,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> AsyncDeliveryTask {
    let sender_namespace = task.sender_namespace.clone();
    let sender_id = route.member.id.clone();
    let body = render_receipt_body(task, outcome, reason_code, reason);
    // A minimal delivery bundle carrying only the original sender as the receipt's
    // target: enough for the worker to resolve the sender's real member (its true
    // transport) and deliver through it, with no co-recipients.
    let bundle = BundleConfiguration {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: sender_namespace.clone(),
        autostart: false,
        groups: Vec::new(),
        members: vec![route.member.clone()],
    };
    AsyncDeliveryTask {
        bundle,
        // A receipt bypasses admission entirely: nothing was accepted for it, so
        // it holds no reservation and stays reportable on its own terms.
        admitted: false,
        // Relay/system origin (`relay@RELAY`), not a peer principal.
        sender_namespace: RELAY_NAMESPACE.to_string(),
        sender: relay_system_sender_member(),
        authenticated_identity: None,
        on_behalf_of: None,
        all_target_sessions: vec![canonical_session_id(
            sender_id.as_str(),
            sender_namespace.as_str(),
        )],
        target_session: sender_id,
        message: body,
        message_id: uuid::Uuid::new_v4().to_string(),
        runtime_directory: route.runtime_directory.clone(),
        payload_mode: DeliveryPayloadMode::EnvelopeMessage,
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: true,
        sender_return_route: None,
    }
}

/// The synthetic relay/system sender member for a receipt envelope. Its
/// `target` is inert: the receipt's transport is built from the receipt's target
/// member (the original sender), never from this sender member.
pub(super) fn relay_system_sender_member() -> BundleMember {
    BundleMember {
        id: TERMINAL_RECEIPT_SENDER_ID.to_string(),
        name: Some(TERMINAL_RECEIPT_SENDER_ID.to_string()),
        working_directory: None,
        target: TargetConfiguration::Ui,
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    }
}

/// Renders the human-readable receipt body naming the original `message_id`, the
/// delivery target, the non-delivered outcome, and any `reason_code`/`reason`, so
/// the sender can correlate it to the `queued` result it received at accept time.
fn render_receipt_body(
    task: &AsyncDeliveryTask,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> String {
    let target = canonical_session_id(
        task.target_session.as_str(),
        task.bundle.bundle_name.as_str(),
    );
    let outcome_word = match outcome {
        SendOutcome::DroppedOnShutdown => "dropped on relay shutdown",
        _ => "failed",
    };
    let mut body = format!(
        "Your message {message_id} to {target} was not delivered ({outcome_word}).",
        message_id = task.message_id,
    );
    if let Some(reason_code) = reason_code {
        body.push_str(&format!(" reason_code={reason_code}"));
    }
    if let Some(reason) = reason {
        body.push_str(&format!(": {reason}"));
    }
    body
}

/// Routes a `delivery_outcome` event for one task back to the sender within its
/// home bundle. Takes the sender/target identity as discrete strings rather than
/// the whole task so ACP completion closures — which hold only cloned per-task
/// fields, not a `&AsyncDeliveryTask` that lives long enough — can emit terminal
/// outcomes once the agent turn finishes.
#[allow(clippy::too_many_arguments)]
pub(in crate::relay::delivery) fn emit_sender_delivery_outcome_event(
    target_namespace: &str,
    sender_namespace: &str,
    sender_session: &str,
    target_session: &str,
    message_id: &str,
    terminal_outcome: SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) {
    let (phase, outcome) = match terminal_outcome {
        SendOutcome::Delivered => ("delivered", Some("success")),
        SendOutcome::DroppedOnShutdown => ("failed", Some("failed")),
        SendOutcome::Failed => ("failed", Some("failed")),
        // A cross-relay peer-unavailable outcome is reported synchronously on the
        // send response, never through this local async terminal-outcome path; the
        // arm is defensive so the outcome maps honestly if it ever reaches here.
        SendOutcome::PeerUnavailable => ("failed", Some("peer_unavailable")),
        // Both terminal, and both distinct from `failed`. `not_submitted` is a
        // sound assertion of non-delivery; `submission_unknown` is the absence of
        // one, so neither may be collapsed into a failure spelling that would
        // claim more than the evidence supports.
        SendOutcome::NotSubmitted => ("not_submitted", Some("not_submitted")),
        SendOutcome::SubmissionUnknown => ("submission_unknown", Some("submission_unknown")),
        SendOutcome::Queued => ("routed", None),
    };
    let mut payload = serde_json::Map::new();
    payload.insert(
        "message_id".to_string(),
        serde_json::Value::String(message_id.to_string()),
    );
    payload.insert(
        "phase".to_string(),
        serde_json::Value::String(phase.to_string()),
    );
    payload.insert(
        "outcome".to_string(),
        outcome
            .map(|value| serde_json::Value::String(value.to_string()))
            .unwrap_or(serde_json::Value::Null),
    );
    if let Some(value) = reason_code {
        payload.insert(
            "reason_code".to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    if let Some(value) = reason {
        payload.insert(
            "reason".to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }

    let event = RelayStreamEvent {
        event_type: "delivery_outcome".to_string(),
        // Describes the target in the TARGET's bundle even though the event
        // routes to the sender's bundle; cross-bundle sends would otherwise
        // misattribute the target to the sender's namespace.
        target_session: canonical_session_id(target_session, target_namespace),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload: serde_json::Value::Object(payload),
    };
    // Route the sender's delivery-outcome event back to the sender within its
    // home bundle, which differs from the target's bundle for cross-bundle sends.
    //
    // Notification runs after the terminal transition and the quota release, and
    // its failure changes neither: the member is already resolved, and refusing
    // to release quota because nobody heard about it would leak the reservation
    // over a reporting problem. So the failure is counted and recorded here
    // rather than propagated.
    match send_event_to_registered_ui(sender_namespace, sender_session, &event) {
        // Nobody was listening. Not a failure — a sender with no attached UI is
        // an ordinary state, and counting it would drown the real signal.
        Ok(StreamEventSendOutcome::Delivered | StreamEventSendOutcome::NoUiEndpoint) => {}
        Ok(StreamEventSendOutcome::Disconnected) => {
            note_outcome_notification_failure("stream", message_id, "sender_ui_disconnected");
        }
        Err(error) => {
            note_outcome_notification_failure("stream", message_id, error.to_string().as_str());
        }
    }
}

/// Running total of terminal-outcome notifications that could not be delivered.
///
/// Process-local and monotonic. It exists so the condition is *countable* rather
/// than only individually observable: a single failed receipt is noise, and a
/// climbing total is a sender that has stopped hearing about its own deliveries.
static OUTCOME_NOTIFICATION_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Counts and records one failed terminal-outcome notification.
///
/// Deliberately returns nothing. The caller has already performed the terminal
/// transition and released admission quota, and there is no recovery available
/// here — the member is resolved whether or not anyone was told. Recording is
/// the whole remedy.
fn note_outcome_notification_failure(channel: &str, message_id: &str, reason: &str) {
    let total = OUTCOME_NOTIFICATION_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
    emit_inscription(
        "relay.send.async.notification_failed",
        &json!({
            "channel": channel,
            "message_id": message_id,
            "reason": reason,
            "notification_failures_total": total,
        }),
    );
}

#[cfg(test)]
mod tests {
    // These tests drive the terminal-resolution chokepoint
    // (`complete_task_outcome` -> `deliver_terminal_outcome_receipt`) directly and
    // observe the routed receipt — or its deliberate absence — on registered worker
    // channels. The chokepoint is crate-private and no public seam can isolate its
    // sender-route and non-recursion behavior: the only end-to-end exerciser is the
    // real-tmux send pipeline, and asserting the *absence* of a receipt there would
    // require reading the very sender pane the failing (non-delivered) case wedges.
    // `terminal_outcome_receipt_routes_to_sender_worker_not_target` proves the
    // receipt is keyed to `sender_return_route`;
    // `a_receipt_outcome_spawns_no_second_receipt` proves the non-recursion gate.
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::relay::delivery::async_worker::registry::{register_worker, unregister_worker};
    use crate::relay::delivery::async_worker::terminal::complete_task_outcome;

    fn tmux_member(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Tmux(crate::configuration::TmuxTargetConfiguration {
                start_command: format!("run-{id}"),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    /// A non-delivered terminal outcome spawns a receipt keyed to the SENDER's
    /// worker — its namespace, runtime directory, and member — and delivers it
    /// there, while a worker registered at the (deliberately different) target key
    /// receives nothing. A receipt built from the target's route would miss the
    /// sender's worker entirely, which is exactly how a cross-bundle receipt would
    /// misroute.
    #[test]
    fn terminal_outcome_receipt_routes_to_sender_worker_not_target() {
        let sender_namespace = "receipt-sender-ns";
        let sender_runtime = "/receipt-sender-rt";
        let sender_member_id = "receipt-sender-mem";
        let target_namespace = "receipt-target-ns";
        let target_runtime = "/receipt-target-rt";
        let target_session = "receipt-target-sess";

        // A live sender worker (the receipt's intended destination) plus a decoy
        // worker at the target key that must NOT receive the receipt. Held receivers
        // keep both channels open.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let sender_owner = register_worker(
            sender_key.clone(),
            sender_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        let target_key =
            build_worker_key(target_namespace, Path::new(target_runtime), target_session);
        let (target_tx, mut target_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            target_key.clone(),
            target_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );

        // The task's delivery context (bundle + runtime) is the target's; its
        // return route is the sender's, with a runtime distinct from the target's.
        let task = AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: target_namespace.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: sender_namespace.to_string(),
            sender: relay_system_sender_member(),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: target_session.to_string(),
            message: "body".to_string(),
            message_id: "orig-message-id".to_string(),
            runtime_directory: PathBuf::from(target_runtime),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: Some(SenderReturnRoute {
                member: tmux_member(sender_member_id),
                runtime_directory: PathBuf::from(sender_runtime),
            }),
        };

        complete_task_outcome(
            &task,
            Ok(SendResult {
                target_session: target_session.to_string(),
                message_id: "orig-message-id".to_string(),
                outcome: SendOutcome::NotSubmitted,
                reason_code: Some("delivery_target_unreachable".to_string()),
                reason: Some(
                    "target could not be reached for longer than the configured dwell".to_string(),
                ),
                details: None,
            }),
        );

        let receipt = sender_rx
            .try_recv()
            .expect("a non-delivered outcome routes a receipt to the sender's worker");
        assert!(receipt.is_receipt, "the routed task is a receipt");
        assert_eq!(receipt.target_session, sender_member_id);
        assert_eq!(receipt.sender_namespace, RELAY_NAMESPACE);
        assert!(
            receipt.message.contains("orig-message-id"),
            "receipt names the original message id: {}",
            receipt.message
        );
        assert!(
            receipt
                .message
                .contains(&canonical_session_id(target_session, target_namespace)),
            "receipt names the delivery target: {}",
            receipt.message
        );
        // The worker at the TARGET key received nothing: the receipt is keyed to the
        // sender, never the target.
        assert!(
            target_rx.try_recv().is_err(),
            "the receipt must not route to a worker at the target key"
        );

        unregister_worker(&sender_key, sender_owner);
        unregister_worker(&target_key, owner);
    }
}

/// A receipt's own terminal outcome spawns no further receipt (non-recursion).
///
/// Its own block rather than a second test in the module above: the one-test-per
/// -inline-block rule is what forces each exception to carry its own argument,
/// and the module above pre-dates it. The sender worker here is live and routable
/// and the outcome is non-delivered — so the ONLY thing that keeps a second
/// receipt off its channel is the `is_receipt` gate at the single spawn site.
#[cfg(test)]
mod receipt_non_recursion_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::relay::delivery::async_worker::registry::{register_worker, unregister_worker};
    use crate::relay::delivery::async_worker::terminal::complete_task_outcome;

    fn tmux_member(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Tmux(crate::configuration::TmuxTargetConfiguration {
                start_command: format!("run-{id}"),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    /// Without the `is_receipt` gate a receipt for a receipt would land here.
    #[test]
    fn a_receipt_outcome_spawns_no_second_receipt() {
        let sender_namespace = "recursion-sender-ns";
        let sender_runtime = "/recursion-sender-rt";
        let sender_member_id = "recursion-sender-mem";

        // A live, routable sender worker: if a receipt were (wrongly) spawned for
        // this already-a-receipt task, it would route here and the assertion below
        // would catch it.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            sender_key.clone(),
            sender_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );

        // The resolving delivery is itself a receipt (`is_receipt: true`) that
        // resolves to a non-delivered outcome — the case that, but for the gate,
        // would recurse.
        let task = AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: sender_namespace.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: sender_namespace.to_string(),
            sender: relay_system_sender_member(),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: sender_member_id.to_string(),
            message: "receipt body".to_string(),
            message_id: "receipt-message-id".to_string(),
            runtime_directory: PathBuf::from(sender_runtime),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: true,
            sender_return_route: Some(SenderReturnRoute {
                member: tmux_member(sender_member_id),
                runtime_directory: PathBuf::from(sender_runtime),
            }),
        };

        complete_task_outcome(
            &task,
            Ok(SendResult {
                target_session: sender_member_id.to_string(),
                message_id: "receipt-message-id".to_string(),
                outcome: SendOutcome::NotSubmitted,
                reason_code: Some("delivery_target_unreachable".to_string()),
                reason: Some(
                    "target could not be reached for longer than the configured dwell".to_string(),
                ),
                details: None,
            }),
        );

        assert!(
            matches!(
                sender_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "a receipt's own non-delivered outcome must not spawn a second receipt; \
             the sender channel must stay connected and empty (a Disconnected here \
             would mean the worker was never live, voiding the proof)"
        );

        unregister_worker(&sender_key, owner);
    }
}
