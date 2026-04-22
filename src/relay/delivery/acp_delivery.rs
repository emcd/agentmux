use std::path::Path;

use serde_json::{Value, json};

use crate::{
    acp::{AcpSnapshotEntry, ReplayEntry, replay_entries_to_snapshot_entries},
    configuration::{AcpTargetConfiguration, BundleMember, TargetConfiguration},
};

use super::acp_client::{AcpRequestError, AcpStdioClient};
use super::acp_state::{
    AcpWorkerReadinessState, append_acp_snapshot_entries, load_persisted_acp_session_id,
    persist_acp_session_id, persist_acp_worker_state, replace_acp_snapshot_entries_from_load,
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
    pub loaded_existing_session: bool,
    pub bootstrap_load_entries: Vec<ReplayEntry>,
}

pub(super) fn bootstrap_acp_worker_runtime(
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, String> {
    let TargetConfiguration::Acp(acp_target) = &target_member.target else {
        return Err("ACP worker bootstrap requires ACP target".to_string());
    };
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return Err("ACP worker bootstrap requires target working directory".to_string());
    };
    let target_session = target_member.id.as_str();
    persist_acp_worker_state(
        runtime_directory,
        target_session,
        target_member.coder_session_id.as_deref(),
        AcpWorkerReadinessState::Initializing,
    )
    .map_err(|reason| format!("persist ACP worker initializing state failed: {reason}"))?;

    let message_id = "acp-worker-bootstrap";
    let mut runtime = initialize_persistent_acp_worker_runtime(
        target_member,
        acp_target,
        working_directory,
        runtime_directory,
        target_session,
        message_id,
    )
    .map_err(|result| {
        let code = result
            .reason_code
            .clone()
            .unwrap_or_else(|| "runtime_startup_failed".to_string());
        let reason = result
            .reason
            .clone()
            .unwrap_or_else(|| "ACP worker bootstrap failed".to_string());
        format!("{code}: {reason}")
    })?;

    if runtime.loaded_existing_session {
        let mut refreshed_entries =
            replay_entries_to_snapshot_entries(runtime.bootstrap_load_entries.as_slice());
        if refreshed_entries.is_empty() {
            let refreshed_lines = runtime.client.take_snapshot_lines();
            refreshed_entries = text_lines_to_update_entries(refreshed_lines.as_slice());
        }
        replace_acp_snapshot_entries_from_load(
            runtime_directory,
            target_session,
            runtime.session_id.as_str(),
            refreshed_entries.as_slice(),
        )
        .map_err(|reason| format!("persist ACP bootstrap snapshot entries failed: {reason}"))?;
    }
    runtime.bootstrap_load_entries.clear();
    persist_acp_worker_state(
        runtime_directory,
        target_session,
        Some(runtime.session_id.as_str()),
        AcpWorkerReadinessState::Available,
    )
    .map_err(|reason| format!("persist ACP worker available state failed: {reason}"))?;
    Ok(runtime)
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
    if target_member.working_directory.is_none() {
        return failed_result(
            target_session,
            message_id,
            "ACP target is missing working directory",
        );
    }
    let runtime_directory = task.runtime_directory.as_path();

    let Some(runtime) = acp_runtime.as_mut() else {
        return failed_result_with_code(
            target_session,
            message_id,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
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
                    runtime_directory,
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
        let mut on_replay_entries = |replay_entries: &[ReplayEntry]| -> Result<(), String> {
            let snapshot_entries = replay_entries_to_snapshot_entries(replay_entries);
            append_acp_snapshot_entries(
                runtime_directory,
                target_member.id.as_str(),
                session_id.as_str(),
                snapshot_entries.as_slice(),
            )
        };
        let prompt_result = runtime.client.prompt(
            session_id.as_str(),
            prompt.as_str(),
            turn_timeout,
            Some(&mut on_dispatched),
            Some(&mut on_replay_entries),
            None,
        );
        let _ = runtime.client.take_snapshot_lines();
        let _ = runtime.client.take_replay_entries();
        match prompt_result {
            Ok(prompt_completion) => {
                first_activity_observed |= prompt_completion.first_activity_observed;
                if let Err(reason) = persist_acp_worker_state(
                    runtime_directory,
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
                    runtime_directory,
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
                    runtime_directory,
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
                    runtime_directory,
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
    runtime_directory: &Path,
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
                runtime_directory,
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
        load_persisted_acp_session_id(runtime_directory, target_member.id.as_str()).map_err(
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

    let mut bootstrap_load_entries = Vec::<ReplayEntry>::new();
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
            bootstrap_load_entries =
                match client.load_session(lifecycle_session_id.as_str(), working_directory) {
                    Ok(entries) => entries,
                    Err(reason) => {
                        let _ = persist_acp_worker_state(
                            runtime_directory,
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
                };
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => match client.new_session(working_directory) {
            Ok(value) => value,
            Err(reason) => {
                let _ = persist_acp_worker_state(
                    runtime_directory,
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
        runtime_directory,
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

    Ok(PersistentAcpWorkerRuntime {
        client,
        session_id,
        loaded_existing_session: matches!(lifecycle, AcpLifecycleSelection::LoadSession),
        bootstrap_load_entries,
    })
}

fn text_lines_to_update_entries(lines: &[String]) -> Vec<AcpSnapshotEntry> {
    lines
        .iter()
        .map(|line| AcpSnapshotEntry::Update {
            update_kind: "text".to_string(),
            lines: vec![line.clone()],
        })
        .collect()
}
