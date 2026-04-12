use std::path::Path;

use serde_json::{Value, json};

use crate::configuration::{AcpTargetConfiguration, BundleMember};

use super::acp_client::{AcpRequestError, AcpStdioClient};
use super::acp_state::{
    AcpWorkerReadinessState, load_persisted_acp_session_id, persist_acp_session_id,
    persist_acp_snapshot_lines, persist_acp_worker_state,
};
use super::results::{
    delivered_in_progress_result, delivered_result, failed_result, failed_result_with_code,
    timeout_result,
};

use super::super::{AsyncDeliveryTask, ChatResult};

pub(super) const ACP_REASON_CODE_TURN_TIMEOUT: &str = "acp_turn_timeout";
pub(super) const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
pub(super) const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
pub(super) const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
pub(super) const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
pub(super) const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
pub(super) const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
pub(super) const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

#[derive(Clone, Copy, Debug)]
enum AcpLifecycleSelection {
    NewSession,
    LoadSession,
}

#[derive(Clone, Debug)]
struct AcpCapabilities {
    load_session: bool,
    prompt_session: bool,
}

pub(super) struct PersistentAcpWorkerRuntime {
    pub client: AcpStdioClient,
    pub session_id: String,
}

pub(super) fn deliver_one_target_acp(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    prompt_batches: Vec<String>,
    target_session: String,
    message_id: String,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> ChatResult {
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return failed_result(
            target_session,
            message_id,
            "ACP target is missing working directory",
        );
    };
    let runtime_socket_path = task.tmux_socket.as_path();

    if acp_runtime.is_none() {
        match initialize_persistent_acp_worker_runtime(
            target_member,
            acp,
            working_directory,
            runtime_socket_path,
            target_session.as_str(),
            message_id.as_str(),
        ) {
            Ok(runtime) => *acp_runtime = Some(runtime),
            Err(result) => return *result,
        }
    }

    let Some(runtime) = acp_runtime.as_mut() else {
        return failed_result(
            target_session,
            message_id,
            "ACP worker runtime was not initialized",
        );
    };

    let turn_timeout = Some(task.quiescence.acp_turn_timeout(acp));
    let mut first_activity_observed = false;
    let completion_sender = task.completion_sender.clone();
    let mut sync_completion_sent = false;
    for prompt in prompt_batches {
        let session_id = runtime.session_id.clone();
        let target_session_for_dispatch = target_session.clone();
        let message_id_for_dispatch = message_id.clone();
        let mut on_dispatched = || {
            if !first_activity_observed {
                first_activity_observed = true;
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(session_id.as_str()),
                    AcpWorkerReadinessState::Busy,
                );
            }
            if sync_completion_sent {
                return;
            }
            let Some(sender) = completion_sender.as_ref() else {
                return;
            };
            sync_completion_sent = true;
            let _ = sender.send(Ok(delivered_in_progress_result(
                target_session_for_dispatch.clone(),
                message_id_for_dispatch.clone(),
            )));
        };
        let mut on_snapshot_lines = |snapshot_lines: &[String]| -> Result<(), String> {
            persist_acp_snapshot_lines(
                runtime_socket_path,
                target_member.id.as_str(),
                session_id.as_str(),
                snapshot_lines,
            )
        };
        let prompt_result = runtime.client.prompt(
            session_id.as_str(),
            prompt.as_str(),
            turn_timeout,
            Some(&mut on_dispatched),
            Some(&mut on_snapshot_lines),
        );
        // Drop any in-memory buffered lines now that updates are persisted
        // incrementally while observed.
        let _ = runtime.client.take_snapshot_lines();
        match prompt_result {
            Ok(prompt_completion) => {
                first_activity_observed |= prompt_completion.first_activity_observed;
                if let Err(reason) = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(session_id.as_str()),
                    AcpWorkerReadinessState::Available,
                ) {
                    return failed_result(
                        target_session,
                        message_id,
                        format!("failed to persist ACP worker state: {reason}"),
                    );
                }
                match prompt_completion.stop_reason.as_str() {
                    "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => {}
                    "cancelled" => {
                        return failed_result_with_code(
                            target_session,
                            message_id,
                            ACP_REASON_CODE_STOP_CANCELLED,
                            "ACP turn completed with stopReason=cancelled",
                            None,
                        );
                    }
                    _ => {
                        *acp_runtime = None;
                        return failed_result(
                            target_session,
                            message_id,
                            format!(
                                "ACP returned unsupported stopReason '{}'",
                                prompt_completion.stop_reason
                            ),
                        );
                    }
                }
            }
            Err(AcpRequestError::Timeout(timeout)) => {
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(session_id.as_str()),
                    AcpWorkerReadinessState::Unavailable,
                );
                *acp_runtime = None;
                return timeout_result(
                    target_session,
                    message_id,
                    Some(ACP_REASON_CODE_TURN_TIMEOUT),
                    format!(
                        "ACP session/prompt timed out after {}ms",
                        timeout.as_millis()
                    ),
                );
            }
            Err(AcpRequestError::ConnectionClosed {
                reason,
                first_activity_observed: observed,
            }) => {
                first_activity_observed |= observed;
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(session_id.as_str()),
                    AcpWorkerReadinessState::Unavailable,
                );
                *acp_runtime = None;
                if first_activity_observed {
                    return delivered_in_progress_result(target_session, message_id);
                }
                return failed_result_with_code(
                    target_session,
                    message_id,
                    ACP_ERROR_CODE_CONNECTION_CLOSED,
                    "ACP connection closed before first activity",
                    Some(json!({
                        "target_session": target_member.id,
                        "reason": reason,
                    })),
                );
            }
            Err(AcpRequestError::Failed(reason)) => {
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(session_id.as_str()),
                    AcpWorkerReadinessState::Unavailable,
                );
                *acp_runtime = None;
                return failed_result_with_code(
                    target_session,
                    message_id,
                    ACP_ERROR_CODE_PROMPT_FAILED,
                    "ACP session/prompt failed",
                    Some(json!({
                        "target_session": target_member.id,
                        "reason": reason,
                    })),
                );
            }
        }
    }

    if first_activity_observed {
        delivered_in_progress_result(target_session, message_id)
    } else {
        delivered_result(target_session, message_id)
    }
}

fn initialize_persistent_acp_worker_runtime(
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    working_directory: &Path,
    runtime_socket_path: &Path,
    target_session: &str,
    message_id: &str,
) -> Result<PersistentAcpWorkerRuntime, Box<ChatResult>> {
    let mut client = match acp.channel {
        crate::configuration::AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return Err(Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    "ACP stdio target requires command",
                )));
            };
            AcpStdioClient::spawn(
                command,
                working_directory,
                &acp.environment
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect::<Vec<_>>(),
            )
            .map_err(|reason| {
                Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    reason,
                ))
            })?
        }
        crate::configuration::AcpChannel::Http => {
            return Err(Box::new(failed_result(
                target_session.to_string(),
                message_id.to_string(),
                "ACP http transport is not implemented",
            )));
        }
    };

    let initialize_result = match client.initialize() {
        Ok(value) => value,
        Err(reason) => {
            let _ = persist_acp_worker_state(
                runtime_socket_path,
                target_member.id.as_str(),
                None,
                AcpWorkerReadinessState::Unavailable,
            );
            return Err(Box::new(failed_result_with_code(
                target_session.to_string(),
                message_id.to_string(),
                ACP_ERROR_CODE_INITIALIZE_FAILED,
                "ACP initialize failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )));
        }
    };

    let capabilities = AcpCapabilities {
        load_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_session: initialize_result
            .get("agentCapabilities")
            .map(|value| {
                value
                    .get("promptSession")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        value
                            .get("promptCapabilities")
                            .is_some_and(serde_json::Value::is_object)
                    })
            })
            .unwrap_or(false),
    };

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        load_persisted_acp_session_id(runtime_socket_path, target_member.id.as_str()).map_err(
            |reason| {
                Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    format!("failed to load persisted ACP session id: {reason}"),
                ))
            },
        )?
    };

    let (lifecycle, lifecycle_session_id) =
        if let Some(configured) = target_member.coder_session_id.as_deref() {
            (AcpLifecycleSelection::LoadSession, configured.to_string())
        } else if let Some(persisted) = persisted_session_id {
            (AcpLifecycleSelection::LoadSession, persisted)
        } else {
            (AcpLifecycleSelection::NewSession, String::new())
        };

    let session_id = match lifecycle {
        AcpLifecycleSelection::LoadSession => {
            if !capabilities.load_session {
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_MISSING_CAPABILITY,
                    "ACP agent does not advertise required load capability",
                    Some(json!({
                        "target_session": target_member.id,
                        "required_capability": "session/load",
                        "reason": "agentCapabilities.loadSession is false or missing",
                    })),
                )));
            }
            if let Err(reason) =
                client.load_session(lifecycle_session_id.as_str(), working_directory)
            {
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    Some(lifecycle_session_id.as_str()),
                    AcpWorkerReadinessState::Unavailable,
                );
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_SESSION_LOAD_FAILED,
                    "ACP session/load failed",
                    Some(json!({
                        "target_session": target_member.id,
                        "session_id": lifecycle_session_id,
                        "reason": reason,
                    })),
                )));
            }
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => match client.new_session(working_directory) {
            Ok(value) => value,
            Err(reason) => {
                let _ = persist_acp_worker_state(
                    runtime_socket_path,
                    target_member.id.as_str(),
                    None,
                    AcpWorkerReadinessState::Unavailable,
                );
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_SESSION_NEW_FAILED,
                    "ACP session/new failed",
                    Some(json!({
                        "target_session": target_member.id,
                        "reason": reason,
                    })),
                )));
            }
        },
    };

    if let Err(reason) = persist_acp_session_id(
        runtime_socket_path,
        target_member.id.as_str(),
        session_id.as_str(),
    ) {
        return Err(Box::new(failed_result(
            target_session.to_string(),
            message_id.to_string(),
            format!("failed to persist ACP session id: {reason}"),
        )));
    }

    if !capabilities.prompt_session {
        return Err(Box::new(failed_result_with_code(
            target_session.to_string(),
            message_id.to_string(),
            ACP_ERROR_CODE_MISSING_CAPABILITY,
            "ACP agent does not advertise required prompt capability",
            Some(json!({
                "target_session": target_member.id,
                "required_capability": "session/prompt",
                "reason": "agentCapabilities.promptSession is false or missing",
            })),
        )));
    }

    Ok(PersistentAcpWorkerRuntime { client, session_id })
}
