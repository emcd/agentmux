use crate::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};

use super::super::super::tmux::{inject_literal_text, inject_prompt, resolve_active_pane_target};
use super::super::super::{
    AsyncDeliveryTask, ChatOutcome, ChatResult, DeliveryPayloadMode, RelayError,
};
use super::super::acp_delivery::{PersistentAcpWorkerRuntime, deliver_one_target_acp};
use super::super::quiescence::{DeliveryWaitError, wait_for_quiescent_pane};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

pub(super) fn deliver_non_ui_target(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> Result<ChatResult, RelayError> {
    match &target_member.target {
        TargetConfiguration::Acp(acp) => Ok(deliver_one_target_acp(
            task,
            target_member,
            acp,
            prompt_batches,
            task.target_session.clone(),
            task.message_id.clone(),
            acp_runtime,
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

fn deliver_one_target_tmux(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
    prompt_batches: Vec<String>,
) -> ChatResult {
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
        None => ChatResult {
            target_session,
            message_id,
            outcome: ChatOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        },
        Some(reason) => ChatResult {
            target_session,
            message_id,
            outcome: ChatOutcome::Failed,
            reason_code: None,
            reason: Some(reason),
            details: None,
        },
    }
}

fn resolve_tmux_pane_target(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
    tmux_socket: &std::path::Path,
) -> Result<String, Box<ChatResult>> {
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
                    ChatResult {
                        target_session: task.target_session.clone(),
                        message_id: task.message_id.clone(),
                        outcome: ChatOutcome::Timeout,
                        reason_code: None,
                        reason: Some(reason),
                        details: None,
                    }
                }
                DeliveryWaitError::Failed { reason } => ChatResult {
                    target_session: task.target_session.clone(),
                    message_id: task.message_id.clone(),
                    outcome: ChatOutcome::Failed,
                    reason_code: None,
                    reason: Some(reason),
                    details: None,
                },
                DeliveryWaitError::Shutdown => ChatResult {
                    target_session: task.target_session.clone(),
                    message_id: task.message_id.clone(),
                    outcome: ChatOutcome::DroppedOnShutdown,
                    reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                    reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                    details: None,
                },
            })
        }),
        DeliveryPayloadMode::RawInput => {
            resolve_active_pane_target(tmux_socket, task.target_session.as_str()).map_err(
                |reason| {
                    Box::new(ChatResult {
                        target_session: task.target_session.clone(),
                        message_id: task.message_id.clone(),
                        outcome: ChatOutcome::Failed,
                        reason_code: Some("tmux_target_unavailable".to_string()),
                        reason: Some(reason),
                        details: None,
                    })
                },
            )
        }
    }
}
