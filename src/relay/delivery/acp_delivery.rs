use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};

use crate::configuration::{AcpTargetConfiguration, BundleMember, TargetConfiguration};

use super::acp_client::{AcpStdioClient, PromptCompletion, PromptDispatchOutcome};
use super::acp_state::{load_persisted_acp_session_id, persist_acp_session_id};
use super::async_worker::{AcpWorkerReadinessState, get_acp_worker_state, set_acp_worker_state};
use super::results::{
    delivered_in_progress_result, delivered_result, failed_result, failed_result_with_code,
};
use super::{
    PermissionEventContext, PermissionResolutionOutcome, enqueue_permission_request,
    wait_for_permission_resolution,
};

use super::super::{AsyncDeliveryTask, ChatResult};

// ACP delivery failure taxonomy:
// - `runtime_acp_initialize_failed` / `_session_new_failed` / `_session_load_failed`:
//   a logical error returned by the ACP agent during bootstrap (well-formed JSON-RPC
//   error response or unexpected response shape).
// - `runtime_acp_prompt_failed`: a logical error returned by the ACP agent during
//   `session/prompt` (JSON-RPC error response or unsupported `stopReason`).
// - `runtime_acp_connection_closed`: ACP stdout reached EOF before any first
//   activity for the active prompt; child likely exited cleanly.
// - `acp_child_unavailable`: ACP child stdin write failed (broken pipe, child
//   crashed, transport-level fault). Worker is transitioned to `Unavailable`.
// - `acp_turn_timeout`: relay-imposed pre-first-activity timeout elapsed.
// - `acp_stop_cancelled`: prompt completed with `stopReason=cancelled`.
// - `validation_missing_acp_capability`: agent did not advertise required
//   capability (`promptSession`, `loadSession`).
// TODO(refactor-acp-background-reader follow-up): `acp_turn_timeout` is no
// longer emitted; relay-side turn timeout is removed (D4). Kept declared
// only so external consumers reading reason codes still have a reference.
#[allow(dead_code)]
pub(super) const ACP_REASON_CODE_TURN_TIMEOUT: &str = "acp_turn_timeout";
pub(super) const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
pub(super) const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
pub(super) const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
pub(super) const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
pub(super) const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
pub(super) const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
pub(super) const ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE: &str = "acp_child_unavailable";
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
    let message_id = "acp-worker-bootstrap";
    let runtime = initialize_persistent_acp_worker_runtime(
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
    Ok(runtime)
}

pub(super) fn deliver_one_target_acp(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    _acp: &AcpTargetConfiguration,
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

    // Fail fast if the background reader already observed a transport
    // failure (Unavailable state). Without this, a dispatch would proceed
    // to write to a dead pipe and bubble up as `acp_child_unavailable`,
    // muddling the "worker is broken" signal. TODO(acp): consider
    // graceful auto-respawn here; see todos/acp/auto_respawn_after_transport_failure.
    if matches!(
        get_acp_worker_state(
            task.bundle.bundle_name.as_str(),
            runtime_directory,
            target_member.id.as_str(),
        ),
        Some(AcpWorkerReadinessState::Unavailable)
    ) {
        return failed_result_with_code(
            target_session,
            message_id,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
        );
    }

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

    debug_assert!(
        prompt_batches.len() == 1,
        "ACP delivery expects exactly one prompt batch; multi-batch requires chaining via on_completion (not implemented)"
    );
    let Some(prompt) = prompt_batches.into_iter().next() else {
        return failed_result(
            target_session,
            message_id,
            "ACP delivery received no prompt batch",
        );
    };

    let permission_context = PermissionEventContext {
        runtime_directory: task.runtime_directory.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        authorized_ui_sessions: task.permission_decider_sessions.clone(),
    };
    let pending_permission_outcome_shared: Arc<Mutex<Option<PermissionResolutionOutcome>>> =
        Arc::new(Mutex::new(None));

    let bundle_name = task.bundle.bundle_name.clone();
    let runtime_directory_owned = task.runtime_directory.clone();
    let target_member_id = target_member.id.clone();
    let session_id = runtime.session_id.clone();

    let dispatch_bundle_name = bundle_name.clone();
    let dispatch_runtime_directory = runtime_directory_owned.clone();
    let dispatch_target_member_id = target_member_id.clone();
    let on_dispatched: crate::acp::DispatchHandler = Box::new(move || {
        set_acp_worker_state(
            dispatch_bundle_name.as_str(),
            dispatch_runtime_directory.as_path(),
            dispatch_target_member_id.as_str(),
            AcpWorkerReadinessState::Busy,
        );
    });

    let permission_context_for_handler = permission_context.clone();
    let message_id_for_handler = message_id.clone();
    let permission_target_member_id = target_member_id.clone();
    let permission_max_pending = task.permission_max_pending;
    let pending_permission_outcome_writer = Arc::clone(&pending_permission_outcome_shared);
    let on_permission_request: crate::acp::PermissionHandler =
        Box::new(move |permission_request: &crate::acp::PermissionRequest| {
            let (response_option_id, outcome) = resolve_acp_permission_request(
                &permission_context_for_handler,
                message_id_for_handler.as_str(),
                permission_target_member_id.as_str(),
                permission_request,
                permission_max_pending,
            );
            *pending_permission_outcome_writer
                .lock()
                .expect("pending_permission_outcome mutex") = Some(outcome);
            response_option_id
        });

    let completion_bundle_name = bundle_name.clone();
    let completion_runtime_directory = runtime_directory_owned.clone();
    let completion_target_member_id = target_member_id.clone();
    let completion_target_session = target_session.clone();
    let completion_message_id = message_id.clone();
    let completion_sender = task.completion_sender.clone();
    let pending_permission_outcome_reader = Arc::clone(&pending_permission_outcome_shared);
    let on_completion: crate::acp::PromptCompletionHandler = Box::new(move |completion| {
        let pending_permission_outcome = pending_permission_outcome_reader
            .lock()
            .expect("pending_permission_outcome mutex")
            .clone();
        let (final_state, final_result) = build_acp_completion_result(
            completion,
            pending_permission_outcome,
            completion_target_session.clone(),
            completion_message_id.clone(),
            completion_target_member_id.as_str(),
        );
        set_acp_worker_state(
            completion_bundle_name.as_str(),
            completion_runtime_directory.as_path(),
            completion_target_member_id.as_str(),
            final_state,
        );
        if let Some(sender) = completion_sender.as_ref() {
            let _ = sender.send(Ok(final_result));
        }
    });

    let outcome = runtime.client.prompt(
        session_id.as_str(),
        prompt.as_str(),
        Some(on_dispatched),
        Some(on_permission_request),
        on_completion,
    );

    match outcome {
        PromptDispatchOutcome::Submitted => {
            delivered_in_progress_result(target_session, message_id)
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt dispatch failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
    }
}

fn build_acp_completion_result(
    completion: PromptCompletion,
    pending_permission_outcome: Option<PermissionResolutionOutcome>,
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> (AcpWorkerReadinessState, ChatResult) {
    if let Some(PermissionResolutionOutcome::Cancelled {
        reason_code,
        reason,
        ..
    }) = pending_permission_outcome
    {
        return (
            AcpWorkerReadinessState::Available,
            failed_result_with_code(
                target_session,
                message_id,
                reason_code.as_str(),
                reason.unwrap_or_else(|| "ACP permission request was cancelled".to_string()),
                Some(json!({
                    "target_session": target_member_id,
                })),
            ),
        );
    }

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => (
                AcpWorkerReadinessState::Available,
                delivered_result(target_session, message_id),
            ),
            "cancelled" => (
                AcpWorkerReadinessState::Available,
                failed_result_with_code(
                    target_session,
                    message_id,
                    ACP_REASON_CODE_STOP_CANCELLED,
                    "ACP turn completed with stopReason=cancelled",
                    None,
                ),
            ),
            other => (
                AcpWorkerReadinessState::Available,
                failed_result(
                    target_session,
                    message_id,
                    format!("ACP returned unsupported stopReason '{other}'"),
                ),
            ),
        },
        PromptCompletion::ProtocolError(reason) => (
            AcpWorkerReadinessState::Available,
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt failed",
                Some(json!({
                    "target_session": target_member_id,
                    "reason": reason,
                })),
            ),
        ),
        PromptCompletion::ConnectionClosed { reason } => (
            AcpWorkerReadinessState::Unavailable,
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_CONNECTION_CLOSED,
                "ACP connection closed before prompt response",
                Some(json!({
                    "target_session": target_member_id,
                    "reason": reason,
                })),
            ),
        ),
    }
}

fn resolve_acp_permission_request(
    permission_context: &PermissionEventContext,
    message_id: &str,
    target_session: &str,
    permission_request: &crate::acp::PermissionRequest,
    permission_max_pending: usize,
) -> (Option<String>, PermissionResolutionOutcome) {
    let requested_details = json!({
        "tool_call_title": permission_request.tool_call_title.clone(),
        "options": permission_request.options.clone(),
        "acp_request_id": permission_request.request_id,
        "raw": permission_request.requested_details.clone(),
    });
    let enqueue = enqueue_permission_request(
        permission_context,
        message_id,
        target_session,
        permission_request.requested_kind.as_str(),
        requested_details,
        permission_request.options.as_slice(),
        permission_max_pending,
    );
    let enqueued = match enqueue {
        Ok(value) => value,
        Err(code) if code == "runtime_permission_queue_full" => {
            return (
                None,
                PermissionResolutionOutcome::Cancelled {
                    decided_by: "relay".to_string(),
                    reason_code: "runtime_permission_queue_full".to_string(),
                    reason: Some("permission queue is full".to_string()),
                },
            );
        }
        Err(_) => {
            return (
                None,
                PermissionResolutionOutcome::Cancelled {
                    decided_by: "relay".to_string(),
                    reason_code: "runtime_permission_queue_unavailable".to_string(),
                    reason: Some("failed to enqueue permission request".to_string()),
                },
            );
        }
    };

    let outcome =
        wait_for_permission_resolution(permission_context, enqueued.permission_request_id.as_str());
    let Ok(outcome) = outcome else {
        return (
            None,
            PermissionResolutionOutcome::Cancelled {
                decided_by: "relay".to_string(),
                reason_code: "runtime_permission_request_cancelled".to_string(),
                reason: Some("failed while waiting for permission decision".to_string()),
            },
        );
    };

    let response_option_id = match &outcome {
        PermissionResolutionOutcome::Selected { option_id, .. } => Some(option_id.clone()),
        PermissionResolutionOutcome::Cancelled { .. } => None,
    };
    (response_option_id, outcome)
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

    Ok(PersistentAcpWorkerRuntime { client, session_id })
}
