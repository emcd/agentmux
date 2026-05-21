use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::BundleMember,
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::super::super::canonical_session_id;
use super::super::super::stream::{
    RelayStreamEvent, broadcast_event_to_bundle_ui, list_registered_ui_sessions_for_bundle,
};
use super::super::super::{AsyncDeliveryTask, ChatResult, RelayError};
use super::super::acp_delivery::{
    ACP_ERROR_CODE_CONNECTION_CLOSED, ACP_ERROR_CODE_INITIALIZE_FAILED,
    ACP_ERROR_CODE_PROMPT_FAILED, ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE, AcpBootstrapError,
    PersistentAcpWorkerRuntime, bootstrap_acp_worker_runtime, respawn_acp_worker_runtime,
};
use super::super::async_worker::{
    AcpWorkerReadinessState, AsyncWorkerKey, get_acp_worker_state,
    install_acp_worker_replay_buffer, set_acp_worker_state,
};
use super::super::permission_state::{PermissionEventContext, invalidate_pending_for_respawn};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
const RESPAWN_INIT_FAILURE_THRESHOLD: u32 = 3;

#[derive(Clone)]
pub(super) struct AcpWorkerBootstrap {
    pub(super) target_member: BundleMember,
    pub(super) runtime_directory: std::path::PathBuf,
}

pub(super) fn spawn_async_delivery_worker(
    key: AsyncWorkerKey,
    receiver: mpsc::Receiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    thread::spawn(move || {
        let acp_context = bootstrap.clone();
        let mut acp_runtime = None::<PersistentAcpWorkerRuntime>;
        let mut respawn_state = AcpRespawnState::new();
        if let Some(bootstrap) = bootstrap {
            set_acp_worker_state(
                key.bundle_name.as_str(),
                bootstrap.runtime_directory.as_path(),
                bootstrap.target_member.id.as_str(),
                AcpWorkerReadinessState::Initializing,
            );
            match bootstrap_acp_worker_runtime(
                bootstrap.runtime_directory.as_path(),
                &bootstrap.target_member,
            ) {
                Ok(runtime) => {
                    install_acp_worker_replay_buffer(
                        key.bundle_name.as_str(),
                        bootstrap.runtime_directory.as_path(),
                        bootstrap.target_member.id.as_str(),
                        runtime.client.replay_buffer_handle(),
                    );
                    set_acp_worker_state(
                        key.bundle_name.as_str(),
                        bootstrap.runtime_directory.as_path(),
                        bootstrap.target_member.id.as_str(),
                        AcpWorkerReadinessState::Available,
                    );
                    acp_runtime = Some(runtime);
                }
                Err(error) => {
                    set_acp_worker_state(
                        key.bundle_name.as_str(),
                        bootstrap.runtime_directory.as_path(),
                        bootstrap.target_member.id.as_str(),
                        AcpWorkerReadinessState::Unavailable,
                    );
                    emit_inscription(
                        "relay.acp.worker.bootstrap_failed",
                        &json!({
                            "bundle_name": key.bundle_name,
                            "target_session": key.target_session,
                            "error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                }
            }
        }
        loop {
            if shutdown_requested() {
                super::super::async_worker::drop_pending_async_tasks_on_shutdown(
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
                super::super::async_worker::complete_task_on_shutdown(&task);
                super::super::async_worker::release_pending_slot(pending.as_ref());
                super::super::async_worker::drop_pending_async_tasks_on_shutdown(
                    &receiver,
                    pending.as_ref(),
                );
                break;
            }

            let outcome =
                super::orchestration::deliver_one_target_with_worker_state(&task, &mut acp_runtime);
            let trigger_reason = classify_respawn_trigger(&outcome);
            super::super::async_worker::complete_task_outcome(&task, outcome);
            super::super::async_worker::release_pending_slot(pending.as_ref());

            // Per-target ACP single-flight: block until the previous prompt
            // is fully complete (background reader fired `on_completion`, or
            // synchronous dispatch failure already cleared the slot) before
            // pulling the next task. `wait_for_prompt_complete()` returns
            // immediately if no prompt was dispatched.
            if let Some(runtime) = acp_runtime.as_ref() {
                runtime.client.wait_for_prompt_complete();
            }

            if let Some(ctx) = acp_context.as_ref() {
                let state = get_acp_worker_state(
                    key.bundle_name.as_str(),
                    ctx.runtime_directory.as_path(),
                    ctx.target_member.id.as_str(),
                );
                if matches!(state, Some(AcpWorkerReadinessState::Unavailable)) {
                    drive_acp_worker_respawn(
                        &key,
                        ctx,
                        trigger_reason,
                        &mut respawn_state,
                        &mut acp_runtime,
                    );
                } else if matches!(
                    state,
                    Some(AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy)
                ) {
                    respawn_state.reset_on_success();
                }
            }
        }
        super::super::async_worker::unregister_worker(&key);
    });
}

struct AcpRespawnState {
    attempt: u32,
    next_backoff_ms: u64,
    last_initialize_failure_reason: Option<String>,
    consecutive_initialize_failures: u32,
}

impl AcpRespawnState {
    fn new() -> Self {
        Self {
            attempt: 0,
            next_backoff_ms: 0,
            last_initialize_failure_reason: None,
            consecutive_initialize_failures: 0,
        }
    }

    fn advance(&mut self) -> Duration {
        let cap = respawn_backoff_cap_ms();
        let backoff = if self.next_backoff_ms == 0 {
            RESPAWN_BACKOFF_INITIAL_MS.min(cap)
        } else {
            self.next_backoff_ms.min(cap)
        };
        self.next_backoff_ms = backoff.saturating_mul(2).min(cap);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(backoff)
    }

    fn record_failure(&mut self, error: &AcpBootstrapError) {
        if error.code == ACP_ERROR_CODE_INITIALIZE_FAILED {
            if self.last_initialize_failure_reason.as_deref() == Some(error.reason.as_str()) {
                self.consecutive_initialize_failures =
                    self.consecutive_initialize_failures.saturating_add(1);
            } else {
                self.last_initialize_failure_reason = Some(error.reason.clone());
                self.consecutive_initialize_failures = 1;
            }
        } else {
            self.last_initialize_failure_reason = None;
            self.consecutive_initialize_failures = 0;
        }
    }

    fn should_give_up(&self) -> bool {
        self.consecutive_initialize_failures >= RESPAWN_INIT_FAILURE_THRESHOLD
    }

    fn reset_on_success(&mut self) {
        self.attempt = 0;
        self.next_backoff_ms = 0;
        self.last_initialize_failure_reason = None;
        self.consecutive_initialize_failures = 0;
    }
}

fn respawn_backoff_cap_ms() -> u64 {
    std::env::var(RESPAWN_BACKOFF_MAX_MS_ENVVAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(RESPAWN_BACKOFF_CAP_DEFAULT_MS)
}

fn classify_respawn_trigger(outcome: &Result<ChatResult, RelayError>) -> &'static str {
    match outcome {
        Ok(result) => match result.reason_code.as_deref() {
            Some(code) if code == ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE => "transport_unavailable",
            Some(code) if code == ACP_ERROR_CODE_PROMPT_FAILED => "serialization_failed",
            Some(code) if code == ACP_ERROR_CODE_CONNECTION_CLOSED => "connection_closed",
            _ => "worker_unavailable",
        },
        Err(_) => "worker_unavailable",
    }
}

fn drive_acp_worker_respawn(
    key: &AsyncWorkerKey,
    ctx: &AcpWorkerBootstrap,
    trigger_reason: &'static str,
    respawn_state: &mut AcpRespawnState,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) {
    // Drop the dead runtime so its child and reader thread are joined before
    // the new child is spawned. Without this, `respawn_acp_worker_runtime`
    // would leave the zombie process unreaped until the worker thread exits.
    *acp_runtime = None;

    loop {
        if shutdown_requested() {
            return;
        }
        let backoff = respawn_state.advance();
        set_acp_worker_state(
            key.bundle_name.as_str(),
            ctx.runtime_directory.as_path(),
            ctx.target_member.id.as_str(),
            AcpWorkerReadinessState::Recovering,
        );
        emit_inscription(
            "relay.acp.respawn.triggered",
            &json!({
                "bundle_name": key.bundle_name,
                "target_session": ctx.target_member.id,
                "attempt": respawn_state.attempt,
                "trigger_reason": trigger_reason,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );
        broadcast_event_to_bundle_ui(
            key.bundle_name.as_str(),
            &acp_respawn_stream_event(
                "acp_worker_respawn_started",
                key.bundle_name.as_str(),
                ctx.target_member.id.as_str(),
                json!({
                    "attempt": respawn_state.attempt,
                    "trigger_reason": trigger_reason,
                    "backoff_ms": backoff.as_millis() as u64,
                }),
            ),
        );

        if !sleep_with_shutdown_gate(backoff) {
            return;
        }

        let permission_context = PermissionEventContext {
            runtime_directory: ctx.runtime_directory.clone(),
            bundle_name: key.bundle_name.clone(),
            authorized_ui_sessions: list_registered_ui_sessions_for_bundle(
                key.bundle_name.as_str(),
            ),
        };
        if let Err(reason) =
            invalidate_pending_for_respawn(&permission_context, ctx.target_member.id.as_str())
        {
            emit_inscription(
                "relay.acp.respawn.permission_invalidate_failed",
                &json!({
                    "bundle_name": key.bundle_name,
                    "target_session": ctx.target_member.id,
                    "reason": reason,
                }),
            );
        }

        match respawn_acp_worker_runtime(
            key.bundle_name.as_str(),
            ctx.runtime_directory.as_path(),
            &ctx.target_member,
        ) {
            Ok(runtime) => {
                set_acp_worker_state(
                    key.bundle_name.as_str(),
                    ctx.runtime_directory.as_path(),
                    ctx.target_member.id.as_str(),
                    AcpWorkerReadinessState::Available,
                );
                emit_inscription(
                    "relay.acp.respawn.succeeded",
                    &json!({
                        "bundle_name": key.bundle_name,
                        "target_session": ctx.target_member.id,
                        "attempt": respawn_state.attempt,
                    }),
                );
                broadcast_event_to_bundle_ui(
                    key.bundle_name.as_str(),
                    &acp_respawn_stream_event(
                        "acp_worker_respawn_completed",
                        key.bundle_name.as_str(),
                        ctx.target_member.id.as_str(),
                        json!({
                            "attempt": respawn_state.attempt,
                            "outcome": "succeeded",
                        }),
                    ),
                );
                *acp_runtime = Some(runtime);
                respawn_state.reset_on_success();
                return;
            }
            Err(error) => {
                respawn_state.record_failure(&error);
                emit_inscription(
                    "relay.acp.respawn.attempt_failed",
                    &json!({
                        "bundle_name": key.bundle_name,
                        "target_session": ctx.target_member.id,
                        "attempt": respawn_state.attempt,
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                if error.is_permanent() || respawn_state.should_give_up() {
                    set_acp_worker_state(
                        key.bundle_name.as_str(),
                        ctx.runtime_directory.as_path(),
                        ctx.target_member.id.as_str(),
                        AcpWorkerReadinessState::Unavailable,
                    );
                    emit_inscription(
                        "relay.acp.respawn.permanent_failure",
                        &json!({
                            "bundle_name": key.bundle_name,
                            "target_session": ctx.target_member.id,
                            "attempts": respawn_state.attempt,
                            "final_error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    broadcast_event_to_bundle_ui(
                        key.bundle_name.as_str(),
                        &acp_respawn_stream_event(
                            "acp_worker_respawn_completed",
                            key.bundle_name.as_str(),
                            ctx.target_member.id.as_str(),
                            json!({
                                "attempts": respawn_state.attempt,
                                "outcome": "permanent_failure",
                                "final_error_code": error.code,
                                "reason": error.reason,
                            }),
                        ),
                    );
                    return;
                }
            }
        }
    }
}

fn sleep_with_shutdown_gate(duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if shutdown_requested() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let poll = remaining.min(Duration::from_millis(RESPAWN_SLEEP_POLL_MS));
        if poll.is_zero() {
            break;
        }
        thread::sleep(poll);
    }
    !shutdown_requested()
}

fn acp_respawn_stream_event(
    event_type: &str,
    bundle_name: &str,
    target_session: &str,
    payload: serde_json::Value,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: event_type.to_string(),
        bundle_name: bundle_name.to_string(),
        target_session: canonical_session_id(target_session, bundle_name),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload,
    }
}
