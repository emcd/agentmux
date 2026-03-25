mod acp_client;
mod acp_delivery;
mod acp_state;
mod async_worker;
mod quiescence;
mod results;
mod ui_delivery;

use std::{sync::mpsc, thread, time::Duration};

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::TargetConfiguration,
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use self::acp_delivery::{PersistentAcpWorkerRuntime, deliver_one_target_acp};
pub(in crate::relay) use self::acp_state::load_acp_snapshot_lines_for_look;
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
