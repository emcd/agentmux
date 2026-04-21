use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::{BundleConfiguration, BundleMember, TargetConfiguration},
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::acp_delivery::{
    PersistentAcpWorkerRuntime, bootstrap_acp_worker_runtime, deliver_one_target_acp,
};
use super::acp_state::{
    ACP_LOOK_PRIME_TIMEOUT_MS, AcpWorkerReadinessState, load_acp_worker_readiness_state,
    load_persisted_acp_session_id, persist_acp_worker_state,
};
use super::quiescence::{DeliveryWaitError, wait_for_quiescent_pane};
use super::ui_delivery::deliver_one_target_ui;

use super::super::stream::{RelayClientClass, resolve_registered_client_class};
use super::super::tmux::inject_prompt;
use super::super::{
    AsyncDeliveryTask, ChatOutcome, ChatResult, ChatStatus, RelayError, SCHEMA_VERSION,
};

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

#[derive(Clone)]
struct AcpWorkerBootstrap {
    target_member: BundleMember,
    runtime_directory: std::path::PathBuf,
}

pub(in crate::relay) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    super::async_worker::wait_for_async_delivery_shutdown(timeout)
}

pub(in crate::relay) fn await_acp_worker_prime_for_look(
    bundle: &BundleConfiguration,
    target_member: &BundleMember,
    runtime_directory: &std::path::Path,
) -> Result<bool, RelayError> {
    if !matches!(target_member.target, TargetConfiguration::Acp(_)) {
        return Ok(false);
    }
    let key = super::async_worker::AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle.bundle_name.clone(),
        target_session: target_member.id.clone(),
    };
    if !super::async_worker::worker_exists(&key)? {
        let persisted_session_id =
            load_persisted_acp_session_id(runtime_directory, target_member.id.as_str()).map_err(
                |cause| {
                    super::super::relay_error(
                        "internal_unexpected_failure",
                        "failed to load persisted ACP session id",
                        Some(json!({
                            "target_session": target_member.id,
                            "cause": cause,
                        })),
                    )
                },
            )?;
        let _ = persist_acp_worker_state(
            runtime_directory,
            target_member.id.as_str(),
            persisted_session_id.as_deref(),
            AcpWorkerReadinessState::Unavailable,
        );
        return Ok(false);
    }
    let deadline = Instant::now() + Duration::from_millis(ACP_LOOK_PRIME_TIMEOUT_MS);
    loop {
        let readiness =
            load_acp_worker_readiness_state(runtime_directory, target_member.id.as_str()).map_err(
                |cause| {
                    super::super::relay_error(
                        "internal_unexpected_failure",
                        "failed to load ACP worker readiness state",
                        Some(json!({
                            "target_session": target_member.id,
                            "cause": cause,
                        })),
                    )
                },
            )?;
        match readiness {
            Some(AcpWorkerReadinessState::Initializing) | None => {
                if Instant::now() >= deadline {
                    return Ok(true);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Some(_) => return Ok(false),
        }
    }
}

pub(in crate::relay) fn initialize_acp_target_for_startup(
    bundle_name: &str,
    runtime_directory: &std::path::Path,
    target_member: &BundleMember,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    if !matches!(target_member.target, TargetConfiguration::Acp(_)) {
        return Ok(());
    }
    if target_member.working_directory.is_none() {
        return Err((
            "runtime_acp_initialize_failed".to_string(),
            "ACP startup requires target working directory".to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    let key = super::async_worker::AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle_name.to_string(),
        target_session: target_member.id.clone(),
    };
    if !super::async_worker::worker_exists(&key).map_err(|error| {
        (
            "internal_unexpected_failure".to_string(),
            "failed to query ACP worker registry".to_string(),
            Some(json!({
                "target_session": target_member.id,
                "cause": error.message,
            })),
        )
    })? {
        let (sender, receiver) = mpsc::channel::<AsyncDeliveryTask>();
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bootstrap = AcpWorkerBootstrap {
            target_member: target_member.clone(),
            runtime_directory: runtime_directory.to_path_buf(),
        };
        if super::async_worker::register_worker_if_absent(
            key.clone(),
            sender,
            pending.clone(),
            true,
        )
        .map_err(|error| {
            (
                "internal_unexpected_failure".to_string(),
                "failed to register ACP worker".to_string(),
                Some(json!({
                    "target_session": target_member.id,
                    "cause": error.message,
                })),
            )
        })? {
            spawn_async_delivery_worker(key, receiver, pending, Some(bootstrap));
        }
    }
    let deadline = Instant::now() + Duration::from_millis(ACP_LOOK_PRIME_TIMEOUT_MS);
    loop {
        let readiness =
            load_acp_worker_readiness_state(runtime_directory, target_member.id.as_str()).map_err(
                |cause| {
                    (
                        "internal_unexpected_failure".to_string(),
                        "failed to load ACP worker readiness state".to_string(),
                        Some(json!({
                            "target_session": target_member.id,
                            "cause": cause,
                        })),
                    )
                },
            )?;
        match readiness {
            Some(AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy) => {
                return Ok(());
            }
            Some(AcpWorkerReadinessState::Unavailable) => {
                return Err((
                    "runtime_acp_worker_unavailable".to_string(),
                    "ACP worker is unavailable after startup".to_string(),
                    Some(json!({
                        "target_session": target_member.id,
                    })),
                ));
            }
            Some(AcpWorkerReadinessState::Initializing) | None => {
                if Instant::now() >= deadline {
                    return Err((
                        "runtime_startup_failed".to_string(),
                        "ACP worker did not become ready during startup".to_string(),
                        Some(json!({
                            "target_session": target_member.id,
                            "timeout_ms": ACP_LOOK_PRIME_TIMEOUT_MS,
                        })),
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub(in crate::relay) fn enqueue_async_delivery(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    enqueue_delivery_task(task)
}

pub(in crate::relay) fn enqueue_sync_delivery(
    mut task: AsyncDeliveryTask,
) -> Result<ChatResult, RelayError> {
    let (sender, receiver) = mpsc::channel::<Result<ChatResult, RelayError>>();
    task.completion_sender = Some(sender);
    enqueue_delivery_task(task)?;
    receiver.recv().map_err(|source| {
        super::super::relay_error(
            "internal_unexpected_failure",
            "failed to receive sync delivery result from worker",
            Some(json!({"cause": source.to_string()})),
        )
    })?
}

fn enqueue_delivery_task(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let bounded_acp_queue = super::async_worker::task_uses_acp_transport(&task)?;
    let key = super::async_worker::AsyncWorkerKey {
        runtime_directory: task.runtime_directory.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        target_session: task.target_session.clone(),
    };
    if bounded_acp_queue && !super::async_worker::worker_exists(&key)? {
        return Err(super::super::relay_error(
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": task.target_session,
            })),
        ));
    }
    match super::async_worker::try_existing_worker(&key, task)? {
        None => Ok(()),
        Some(task) => {
            if bounded_acp_queue {
                return Err(super::super::relay_error(
                    "runtime_acp_worker_unavailable",
                    "ACP worker is unavailable for target session",
                    Some(json!({
                        "target_session": task.target_session,
                    })),
                ));
            }
            let (sender, receiver) = mpsc::channel::<AsyncDeliveryTask>();
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            if bounded_acp_queue && !super::async_worker::reserve_acp_pending_slot(pending.as_ref())
            {
                return Err(super::super::relay_error(
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
                    super::async_worker::release_pending_slot(pending.as_ref());
                }
                super::super::relay_error(
                    "internal_unexpected_failure",
                    "failed to enqueue async delivery task",
                    Some(json!({"cause": source.to_string()})),
                )
            })?;
            spawn_async_delivery_worker(key.clone(), receiver, pending.clone(), None);
            super::async_worker::register_worker(key, sender, pending, bounded_acp_queue);
            Ok(())
        }
    }
}

pub(in crate::relay) fn deliver_one_target(
    task: &AsyncDeliveryTask,
) -> Result<ChatResult, RelayError> {
    let mut acp_runtime = None;
    deliver_one_target_with_worker_state(task, &mut acp_runtime)
}

pub(in crate::relay) fn deliver_one_target_with_worker_state(
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
        .find(|member| member.id == target_session);
    if target_member.is_none() && !task.target_is_ui {
        return Err(super::super::relay_error(
            "internal_unexpected_failure",
            "resolved target member is missing from bundle configuration",
            Some(json!({"target_session": target_session})),
        ));
    }
    let cc_sessions = all_target_sessions
        .iter()
        .filter(|candidate| **candidate != target_session)
        .cloned()
        .collect::<Vec<_>>();
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
        cc_sessions: if cc_sessions.is_empty() {
            None
        } else {
            Some(cc_sessions.clone())
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
            session_name: target_session.clone(),
            display_name: target_member.and_then(|member| member.name.clone()),
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
    if task.target_is_ui {
        return Ok(deliver_one_target_ui(
            task,
            sender.id.as_str(),
            cc_sessions.as_slice(),
            target_session,
            message_id,
            message,
        ));
    }
    let prompt_batches = batch_envelopes(&[envelope], batch_settings);
    let resolved_client_class =
        resolve_registered_client_class(bundle.bundle_name.as_str(), target_session.as_str())
            .map_err(|source| {
                super::super::relay_error(
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
            sender.id.as_str(),
            cc_sessions.as_slice(),
            target_session,
            message_id,
            message,
        ));
    }

    let non_ui_target_member = target_member.expect("non-UI target_member must exist");
    match &non_ui_target_member.target {
        TargetConfiguration::Acp(acp) => Ok(deliver_one_target_acp(
            task,
            non_ui_target_member,
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

pub(in crate::relay) fn aggregate_chat_status(results: &[ChatResult]) -> ChatStatus {
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

pub(in crate::relay) fn prompt_batch_settings() -> PromptBatchSettings {
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
    key: super::async_worker::AsyncWorkerKey,
    receiver: mpsc::Receiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    thread::spawn(move || {
        let mut acp_runtime = None::<PersistentAcpWorkerRuntime>;
        if let Some(bootstrap) = bootstrap {
            match bootstrap_acp_worker_runtime(
                bootstrap.runtime_directory.as_path(),
                &bootstrap.target_member,
            ) {
                Ok(runtime) => acp_runtime = Some(runtime),
                Err(reason) => {
                    emit_inscription(
                        "relay.acp.worker.bootstrap_failed",
                        &json!({
                            "bundle_name": key.bundle_name,
                            "target_session": key.target_session,
                            "reason": reason,
                        }),
                    );
                }
            }
        }
        loop {
            if shutdown_requested() {
                super::async_worker::drop_pending_async_tasks_on_shutdown(
                    &receiver,
                    pending.as_ref(),
                );
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
                super::async_worker::complete_task_on_shutdown(&task);
                super::async_worker::release_pending_slot(pending.as_ref());
                super::async_worker::drop_pending_async_tasks_on_shutdown(
                    &receiver,
                    pending.as_ref(),
                );
                break;
            }

            let outcome = deliver_one_target_with_worker_state(&task, &mut acp_runtime);
            super::async_worker::complete_task_outcome(&task, outcome);
            super::async_worker::release_pending_slot(pending.as_ref());
        }
        super::async_worker::unregister_worker(&key);
    });
}
