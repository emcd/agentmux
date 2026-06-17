use crate::acp::AcpTransport;
use crate::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
use crate::transports::{DeliveryContext, DeliveryEnvelope, DeliveryResult, Transport};

use super::super::super::startup_state::note_session_served_successfully;
use super::super::super::tmux::{inject_literal_text, inject_prompt, resolve_active_pane_target};
use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendOutcome, SendResult,
};
use super::super::quiescence::{DeliveryWaitError, wait_for_quiescent_pane};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

pub(super) fn deliver_non_ui_target(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Result<SendResult, RelayError> {
    match &target_member.target {
        TargetConfiguration::Acp(_) => Ok(deliver_acp_combined(
            task,
            target_member,
            prompt_batches,
            acp_transport,
        )),
        TargetConfiguration::Tmux(tmux_target) => {
            Ok(deliver_one_target_tmux(task, tmux_target, prompt_batches))
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            Err(super::super::super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            ))
        }
    }
}

/// Delivers a coalesced envelope batch in a single transport call.
///
/// All tasks in `batch` share the same target; the rendered `prompt_batches`
/// represent the combined envelopes. Returns one outcome per task aligned
/// with the slice. For tmux the single transport outcome (success or the
/// reason the K-th paste failed) is replicated per-task with each task's
/// own `message_id`. For ACP the synchronous return is a `delivered_in_progress`
/// per task; the final outcome is delivered later via `on_completion`
/// (fanned out inside `deliver_batch_target_acp`).
///
/// `pre_resolved_pane` lets the worker loop hoist the tmux quiescence wait so
/// post-quiescence task arrivals can be drained into the batch before paste.
/// When `Some` for a tmux target, this skips the in-transport wait + pane
/// resolution and pastes against the supplied pane directly. ACP/UI/Pubsub
/// targets ignore the value.
pub(super) fn deliver_non_ui_target_batch(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    pre_resolved_pane: Option<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Vec<Result<SendResult, RelayError>> {
    debug_assert!(!batch.is_empty());
    match &target_member.target {
        TargetConfiguration::Acp(_) => {
            deliver_acp_batch_via_transport(batch, target_member, prompt_batches, acp_transport)
                .into_iter()
                .map(Ok)
                .collect()
        }
        TargetConfiguration::Tmux(tmux_target) => {
            deliver_batch_target_tmux(batch, tmux_target, prompt_batches, pre_resolved_pane)
                .into_iter()
                .map(Ok)
                .collect()
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            let error = super::super::super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            );
            batch.iter().map(|_| Err(error.clone())).collect()
        }
    }
}

/// Delivers a coalesced ACP batch through the per-target [`AcpTransport`].
///
/// ACP coalescing is relay-side: the worker already combined the batch into a
/// single rendered prompt, so the transport receives one envelope, blocks to
/// terminal, and returns one outcome. That single outcome is replicated across
/// every task in `batch` (each keeps its own `message_id`/`target_session`),
/// matching the tmux fan-out shape.
fn deliver_acp_batch_via_transport(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Vec<SendResult> {
    let head = &batch[0];
    let outcome = deliver_acp_combined(head, target_member, prompt_batches, acp_transport);
    batch
        .iter()
        .map(|task| SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: outcome.outcome.clone(),
            reason_code: outcome.reason_code.clone(),
            reason: outcome.reason.clone(),
            details: outcome.details.clone(),
        })
        .collect()
}

/// Submits one combined ACP prompt via the transport and converts the single
/// terminal outcome into a [`SendResult`]. Handles the no-transport
/// (bootstrap-failed) and empty-prompt cases the same way the previous in-relay
/// path did.
fn deliver_acp_combined(
    head: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> SendResult {
    let target_session = head.target_session.clone();
    let message_id = head.message_id.clone();

    let Some(transport) = acp_transport.as_mut() else {
        return SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: Some("runtime_acp_worker_unavailable".to_string()),
            reason: Some("ACP worker is unavailable for target session".to_string()),
            details: Some(serde_json::json!({ "target_session": target_member.id })),
        };
    };
    let Some(prompt) = prompt_batches.into_iter().next() else {
        return SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: None,
            reason: Some("ACP delivery received no prompt batch".to_string()),
            details: None,
        };
    };

    let envelope = DeliveryEnvelope {
        message_id: message_id.clone(),
        payload_mode: head.payload_mode,
        rendered: prompt,
        append_enter: head.append_enter,
    };
    let context = DeliveryContext {
        target_session: target_session.clone(),
        runtime_directory: head.runtime_directory.clone(),
        target_member: target_member.clone(),
        pre_resolved_target: None,
        choices_pending_max: head.choices_max_pending,
        choice_decider_sessions: head.choice_decider_sessions.clone(),
    };
    let result = transport.deliver(vec![envelope], &context);
    single_outcome_to_send_result(result, target_session, message_id)
}

fn single_outcome_to_send_result(
    result: DeliveryResult,
    target_session: String,
    message_id: String,
) -> SendResult {
    match result.outcomes.into_iter().next() {
        Some(outcome) => SendResult {
            target_session: outcome.target_session,
            message_id: outcome.message_id,
            outcome: outcome.outcome,
            reason_code: outcome.reason_code,
            reason: outcome.reason,
            details: outcome.details,
        },
        None => SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: Some("internal_unexpected_failure".to_string()),
            reason: Some("ACP delivery produced no outcome".to_string()),
            details: None,
        },
    }
}

/// Worker-loop entry for the tmux quiescence hoist: waits for the head task's
/// pane to become quiescent and returns the resolved pane target on success.
/// Returns the per-batch failure template on timeout, shutdown, or pane
/// unavailability — the worker fans it out to every task in the coalesced
/// batch. Caller must have established that the head task is envelope-mode
/// and targets a tmux session.
pub(super) fn prepare_tmux_pane_for_envelope_head(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
) -> Result<String, Box<SendResult>> {
    debug_assert!(matches!(
        task.payload_mode,
        DeliveryPayloadMode::EnvelopeMessage
    ));
    let tmux_socket_path = crate::runtime::paths::tmux_socket_path_for_runtime_directory(
        task.runtime_directory.as_path(),
    );
    let tmux_socket = tmux_socket_path.as_path();
    resolve_tmux_pane_target(task, tmux_target, tmux_socket)
}

fn deliver_one_target_tmux(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
    prompt_batches: Vec<String>,
) -> SendResult {
    let target_session = task.target_session.clone();
    let message_id = task.message_id.clone();
    let tmux_socket_path = crate::runtime::paths::tmux_socket_path_for_runtime_directory(
        task.runtime_directory.as_path(),
    );
    let tmux_socket = tmux_socket_path.as_path();

    let pane_target = match resolve_tmux_pane_target(task, tmux_target, tmux_socket) {
        Ok(pane_target) => pane_target,
        Err(result) => return *result,
    };

    let failed_reason = match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let mut failed_reason = None::<String>;
            for prompt in prompt_batches {
                if let Err(reason) = inject_prompt(tmux_socket, &pane_target, &prompt) {
                    failed_reason = Some(reason);
                    break;
                }
            }
            failed_reason
        }
        DeliveryPayloadMode::RawInput => inject_literal_text(
            tmux_socket,
            &pane_target,
            task.message.as_str(),
            task.append_enter,
        )
        .err(),
    };
    match failed_reason {
        None => {
            let _ = note_session_served_successfully(
                task.runtime_directory.as_path(),
                target_session.as_str(),
            );
            SendResult {
                target_session,
                message_id,
                outcome: SendOutcome::Delivered,
                reason_code: None,
                reason: None,
                details: None,
            }
        }
        Some(reason) => SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: None,
            reason: Some(reason),
            details: None,
        },
    }
}

/// Tmux delivery for a coalesced envelope batch. Resolves the pane and waits
/// for quiescence ONCE on the head task, then paste-buffers each rendered
/// prompt batch sequentially. The single delivery outcome (success or the
/// reason the K-th paste failed) is fanned out to every task in `batch`
/// using each task's own `message_id` and `target_session`.
///
/// When the worker loop has already proven the pane quiescent (post-quiescence
/// drain path), `pre_resolved_pane` is supplied and both the wait and the
/// pane-target lookup are skipped here.
fn deliver_batch_target_tmux(
    batch: &[AsyncDeliveryTask],
    tmux_target: &TmuxTargetConfiguration,
    prompt_batches: Vec<String>,
    pre_resolved_pane: Option<String>,
) -> Vec<SendResult> {
    let head = &batch[0];
    let tmux_socket_path = crate::runtime::paths::tmux_socket_path_for_runtime_directory(
        head.runtime_directory.as_path(),
    );
    let tmux_socket = tmux_socket_path.as_path();

    let pane_target = match pre_resolved_pane {
        Some(pane_target) => pane_target,
        None => match resolve_tmux_pane_target(head, tmux_target, tmux_socket) {
            Ok(pane_target) => pane_target,
            Err(result) => {
                // Quiescence / pane-resolution failure: every task in the batch
                // shares the outcome (re-built per-task so message_id /
                // target_session correlate with each original send call).
                // `Box<SendResult>` carries the head's correlation values;
                // replicate the variant fields.
                return replicate_outcome_for_batch(batch, *result);
            }
        },
    };

    let mut failed_reason = None::<String>;
    for prompt in prompt_batches {
        if let Err(reason) = inject_prompt(tmux_socket, &pane_target, &prompt) {
            failed_reason = Some(reason);
            break;
        }
    }
    batch
        .iter()
        .map(|task| match &failed_reason {
            None => {
                let _ = note_session_served_successfully(
                    task.runtime_directory.as_path(),
                    task.target_session.as_str(),
                );
                SendResult {
                    target_session: task.target_session.clone(),
                    message_id: task.message_id.clone(),
                    outcome: SendOutcome::Delivered,
                    reason_code: None,
                    reason: None,
                    details: None,
                }
            }
            Some(reason) => SendResult {
                target_session: task.target_session.clone(),
                message_id: task.message_id.clone(),
                outcome: SendOutcome::Failed,
                reason_code: None,
                reason: Some(reason.clone()),
                details: None,
            },
        })
        .collect()
}

// Reproduces a head-derived SendResult for every task in the batch, swapping
// in each task's own correlation fields. Used when a quiescence wait or pane
// resolution fails before the actual paste begins: there is one underlying
// reason but N callers need their own per-task result.
fn replicate_outcome_for_batch(
    batch: &[AsyncDeliveryTask],
    template: SendResult,
) -> Vec<SendResult> {
    batch
        .iter()
        .map(|task| SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: template.outcome.clone(),
            reason_code: template.reason_code.clone(),
            reason: template.reason.clone(),
            details: template.details.clone(),
        })
        .collect()
}

fn resolve_tmux_pane_target(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
    tmux_socket: &std::path::Path,
) -> Result<String, Box<SendResult>> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => wait_for_quiescent_pane(
            tmux_socket,
            task.target_session.as_str(),
            task.quiescence,
            tmux_target.prompt_readiness.as_ref(),
        )
        .map_err(|error| {
            Box::new(match error {
                DeliveryWaitError::Timeout {
                    timeout,
                    readiness_mismatch,
                    mismatch_reason,
                } => {
                    let reason = if readiness_mismatch {
                        let detail = mismatch_reason
                            .map(|value| format!(": {value}"))
                            .unwrap_or_default();
                        format!(
                            "prompt readiness did not match before timeout after {}ms{}",
                            timeout.as_millis(),
                            detail
                        )
                    } else {
                        format!("quiescence wait timed out after {}ms", timeout.as_millis())
                    };
                    SendResult {
                        target_session: task.target_session.clone(),
                        message_id: task.message_id.clone(),
                        outcome: SendOutcome::Timeout,
                        reason_code: None,
                        reason: Some(reason),
                        details: None,
                    }
                }
                DeliveryWaitError::Failed { reason } => SendResult {
                    target_session: task.target_session.clone(),
                    message_id: task.message_id.clone(),
                    outcome: SendOutcome::Failed,
                    reason_code: None,
                    reason: Some(reason),
                    details: None,
                },
                DeliveryWaitError::Shutdown => SendResult {
                    target_session: task.target_session.clone(),
                    message_id: task.message_id.clone(),
                    outcome: SendOutcome::DroppedOnShutdown,
                    reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                    reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                    details: None,
                },
            })
        }),
        DeliveryPayloadMode::RawInput => {
            resolve_active_pane_target(tmux_socket, task.target_session.as_str()).map_err(
                |reason| {
                    Box::new(SendResult {
                        target_session: task.target_session.clone(),
                        message_id: task.message_id.clone(),
                        outcome: SendOutcome::Failed,
                        reason_code: Some("tmux_target_unavailable".to_string()),
                        reason: Some(reason),
                        details: None,
                    })
                },
            )
        }
    }
}
