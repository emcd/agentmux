mod acp_client;
mod acp_state;
mod async_worker;
mod quiescence;
mod ui_delivery;

use std::{path::Path, sync::mpsc, thread, time::Duration};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::{AcpTargetConfiguration, TargetConfiguration},
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use self::acp_client::{AcpRequestError, AcpStdioClient};
pub(in crate::relay) use self::acp_state::load_acp_snapshot_lines_for_look;
use self::acp_state::{
    AcpWorkerReadinessState, load_persisted_acp_session_id, persist_acp_session_id,
    persist_acp_snapshot_lines, persist_acp_worker_state,
};
use self::quiescence::wait_for_quiescent_pane;
pub(in crate::relay) use self::quiescence::{DeliveryWaitError, QuiescenceOptions};
use self::ui_delivery::deliver_one_target_ui;

use super::stream::{RelayClientClass, resolve_registered_client_class};
use super::tmux::inject_prompt;
use super::{AsyncDeliveryTask, ChatOutcome, ChatResult, ChatStatus, RelayError, SCHEMA_VERSION};

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const ACP_REASON_CODE_TURN_TIMEOUT: &str = "acp_turn_timeout";
const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
const ACP_DELIVERY_PHASE_ACCEPTED_IN_PROGRESS: &str = "accepted_in_progress";
const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

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

struct PersistentAcpWorkerRuntime {
    client: AcpStdioClient,
    session_id: String,
}

pub(super) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    async_worker::wait_for_async_delivery_shutdown(timeout)
}

pub(super) fn enqueue_async_delivery(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    enqueue_delivery_task(task)
}

pub(super) fn enqueue_sync_delivery(mut task: AsyncDeliveryTask) -> Result<ChatResult, RelayError> {
    let (sender, receiver) = mpsc::channel::<Result<ChatResult, RelayError>>();
    task.completion_sender = Some(sender);
    enqueue_delivery_task(task)?;
    receiver.recv().map_err(|source| {
        super::relay_error(
            "internal_unexpected_failure",
            "failed to receive sync delivery result from worker",
            Some(json!({"cause": source.to_string()})),
        )
    })?
}

fn enqueue_delivery_task(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let bounded_acp_queue = async_worker::task_uses_acp_transport(&task)?;
    let key = async_worker::AsyncWorkerKey {
        tmux_socket: task.tmux_socket.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        target_session: task.target_session.clone(),
    };
    match async_worker::try_existing_worker(&key, task)? {
        None => Ok(()),
        Some(task) => {
            let (sender, receiver) = mpsc::channel::<AsyncDeliveryTask>();
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            if bounded_acp_queue && !async_worker::reserve_acp_pending_slot(pending.as_ref()) {
                return Err(super::relay_error(
                    "runtime_acp_queue_full",
                    "ACP worker queue is full",
                    Some(json!({
                        "target_session": task.target_session,
                        "max_pending": 64,
                    })),
                ));
            }
            sender.send(task).map_err(|source| {
                if bounded_acp_queue {
                    async_worker::release_pending_slot(pending.as_ref());
                }
                super::relay_error(
                    "internal_unexpected_failure",
                    "failed to enqueue async delivery task",
                    Some(json!({"cause": source.to_string()})),
                )
            })?;
            spawn_async_delivery_worker(key.clone(), receiver, pending.clone());
            async_worker::register_worker(key, sender, pending, bounded_acp_queue);
            Ok(())
        }
    }
}

pub(super) fn deliver_one_target(task: &AsyncDeliveryTask) -> Result<ChatResult, RelayError> {
    let mut acp_runtime = None;
    deliver_one_target_with_worker_state(task, &mut acp_runtime)
}

fn deliver_one_target_with_worker_state(
    task: &AsyncDeliveryTask,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> Result<ChatResult, RelayError> {
    let bundle = &task.bundle;
    let sender = &task.sender;
    let all_target_sessions = &task.all_target_sessions;
    let target_session = task.target_session.clone();
    let message = task.message.as_str();
    let message_id = task.message_id.clone();
    let tmux_socket = task.tmux_socket.as_path();
    let quiescence = task.quiescence;
    let batch_settings = task.batch_settings;
    let created_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let target_member = bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
        .ok_or_else(|| {
            super::relay_error(
                "internal_unexpected_failure",
                "resolved target member is missing from bundle configuration",
                Some(json!({"target_session": target_session})),
            )
        })?;
    let cc_members = all_target_sessions
        .iter()
        .filter(|candidate| **candidate != target_session)
        .filter_map(|session_name| {
            bundle
                .members
                .iter()
                .find(|member| member.id == *session_name)
        })
        .cloned()
        .collect::<Vec<_>>();

    let manifest = ManifestPreamble {
        schema_version: SCHEMA_VERSION.to_string(),
        message_id: message_id.clone(),
        bundle_name: bundle.bundle_name.clone(),
        sender_session: sender.id.clone(),
        target_sessions: vec![target_session.clone()],
        cc_sessions: if cc_members.is_empty() {
            None
        } else {
            Some(
                cc_members
                    .iter()
                    .map(|member| member.id.clone())
                    .collect::<Vec<_>>(),
            )
        },
        created_at,
    };
    emit_inscription(
        "relay.chat.envelope.metadata",
        &json!({
            "schema_version": manifest.schema_version,
            "message_id": manifest.message_id,
            "bundle_name": manifest.bundle_name,
            "sender_session": manifest.sender_session,
            "target_sessions": manifest.target_sessions,
            "cc_sessions": manifest.cc_sessions,
            "created_at": manifest.created_at,
        }),
    );
    let envelope = render_envelope(&EnvelopeRenderInput {
        manifest,
        from: AddressIdentity {
            session_name: sender.id.clone(),
            display_name: sender.name.clone(),
        },
        to: vec![AddressIdentity {
            session_name: target_member.id.clone(),
            display_name: target_member.name.clone(),
        }],
        cc: cc_members
            .iter()
            .map(|member| AddressIdentity {
                session_name: member.id.clone(),
                display_name: member.name.clone(),
            })
            .collect::<Vec<_>>(),
        subject: None,
        body: message.to_string(),
    });
    let prompt_batches = batch_envelopes(&[envelope], batch_settings);
    let resolved_client_class =
        resolve_registered_client_class(bundle.bundle_name.as_str(), target_session.as_str())
            .map_err(|source| {
                super::relay_error(
                    "internal_unexpected_failure",
                    "failed to resolve relay stream endpoint class",
                    Some(json!({
                        "bundle_name": bundle.bundle_name,
                        "target_session": target_session,
                        "cause": source.to_string(),
                    })),
                )
            })?;
    if matches!(resolved_client_class, Some(RelayClientClass::Ui)) {
        return Ok(deliver_one_target_ui(
            task,
            sender,
            &cc_members,
            target_session,
            message_id,
            message,
        ));
    }

    match &target_member.target {
        TargetConfiguration::Acp(acp) => Ok(deliver_one_target_acp(
            task,
            target_member,
            acp,
            prompt_batches,
            target_session,
            message_id,
            acp_runtime,
        )),
        TargetConfiguration::Tmux(tmux_target) => match wait_for_quiescent_pane(
            tmux_socket,
            &target_session,
            quiescence,
            tmux_target.prompt_readiness.as_ref(),
        ) {
            Ok(pane_target) => {
                let mut failed_reason = None::<String>;
                for prompt in prompt_batches {
                    if let Err(reason) = inject_prompt(tmux_socket, &pane_target, &prompt) {
                        failed_reason = Some(reason);
                        break;
                    }
                }
                match failed_reason {
                    None => Ok(ChatResult {
                        target_session,
                        message_id,
                        outcome: ChatOutcome::Delivered,
                        reason_code: None,
                        reason: None,
                        details: None,
                    }),
                    Some(reason) => Ok(ChatResult {
                        target_session,
                        message_id,
                        outcome: ChatOutcome::Failed,
                        reason_code: None,
                        reason: Some(reason),
                        details: None,
                    }),
                }
            }
            Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
                mismatch_reason,
            }) => {
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
                Ok(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Timeout,
                    reason_code: None,
                    reason: Some(reason),
                    details: None,
                })
            }
            Err(DeliveryWaitError::Failed { reason }) => Ok(ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::Failed,
                reason_code: None,
                reason: Some(reason),
                details: None,
            }),
            Err(DeliveryWaitError::Shutdown) => Ok(ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            }),
        },
    }
}

fn delivered_result(target_session: String, message_id: String) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn delivered_in_progress_result(target_session: String, message_id: String) -> ChatResult {
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

fn failed_result(
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

fn failed_result_with_code(
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

fn timeout_result(
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

fn deliver_one_target_acp(
    task: &AsyncDeliveryTask,
    target_member: &crate::configuration::BundleMember,
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
    for prompt in prompt_batches {
        let session_id = runtime.session_id.clone();
        let mut on_first_activity = || {
            if first_activity_observed {
                return;
            }
            first_activity_observed = true;
            let _ = persist_acp_worker_state(
                runtime_socket_path,
                target_member.id.as_str(),
                Some(session_id.as_str()),
                AcpWorkerReadinessState::Busy,
            );
        };
        let prompt_result = runtime.client.prompt(
            session_id.as_str(),
            prompt.as_str(),
            turn_timeout,
            Some(&mut on_first_activity),
        );
        let prompt_snapshot_lines = runtime.client.take_snapshot_lines();
        if let Err(reason) = persist_acp_snapshot_lines(
            runtime_socket_path,
            target_member.id.as_str(),
            session_id.as_str(),
            prompt_snapshot_lines.as_slice(),
        ) {
            return failed_result(
                target_session,
                message_id,
                format!("failed to persist ACP look snapshot state: {reason}"),
            );
        }
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
    target_member: &crate::configuration::BundleMember,
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
            AcpStdioClient::spawn(command, working_directory).map_err(|reason| {
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

pub(super) fn aggregate_chat_status(results: &[ChatResult]) -> ChatStatus {
    let delivered = results
        .iter()
        .filter(|result| result.outcome == ChatOutcome::Delivered)
        .count();
    if delivered == results.len() {
        return ChatStatus::Success;
    }
    if delivered > 0 {
        return ChatStatus::Partial;
    }
    ChatStatus::Failure
}

pub(super) fn prompt_batch_settings() -> PromptBatchSettings {
    let max_prompt_tokens = std::env::var(PROMPT_TOKENS_MAX_ENVVAR)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(PromptBatchSettings::default().max_prompt_tokens);
    let tokenizer_profile = std::env::var(TOKENIZER_PROFILE_ENVVAR)
        .ok()
        .as_deref()
        .and_then(parse_tokenizer_profile)
        .unwrap_or_default();
    PromptBatchSettings {
        max_prompt_tokens,
        tokenizer_profile,
    }
}

fn spawn_async_delivery_worker(
    key: async_worker::AsyncWorkerKey,
    receiver: mpsc::Receiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    thread::spawn(move || {
        let mut acp_runtime = None::<PersistentAcpWorkerRuntime>;
        loop {
            if shutdown_requested() {
                async_worker::drop_pending_async_tasks_on_shutdown(&receiver, pending.as_ref());
                break;
            }
            let received =
                receiver.recv_timeout(Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS));
            let task = match received {
                Ok(task) => task,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if shutdown_requested() {
                async_worker::complete_task_on_shutdown(&task);
                async_worker::release_pending_slot(pending.as_ref());
                async_worker::drop_pending_async_tasks_on_shutdown(&receiver, pending.as_ref());
                break;
            }

            let outcome = deliver_one_target_with_worker_state(&task, &mut acp_runtime);
            async_worker::complete_task_outcome(&task, outcome);
            async_worker::release_pending_slot(pending.as_ref());
        }
        async_worker::unregister_worker(&key);
    });
}
