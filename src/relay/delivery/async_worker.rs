use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, error::SendError};

use crate::configuration::TargetConfiguration;
use crate::runtime::{inscriptions::emit_inscription, signals::shutdown_requested};
use crate::transports::{AcpWorkerReadinessState, OutputView};

use super::super::stream::{RelayStreamEvent, send_event_to_registered_ui};
use super::super::{AsyncDeliveryTask, RelayError, SendOutcome, SendResult, canonical_session_id};

use std::path::{Path, PathBuf};

const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const ACP_ERROR_CODE_QUEUE_FULL: &str = "runtime_acp_queue_full";
const ACP_MAX_PENDING: usize = 64;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct AsyncWorkerKey {
    pub runtime_directory: PathBuf,
    pub bundle_name: String,
    pub target_session: String,
}

#[derive(Default)]
pub(super) struct AsyncDeliveryRegistry {
    pub workers: Mutex<HashMap<AsyncWorkerKey, AsyncWorkerEntry>>,
}

pub(super) struct AsyncWorkerEntry {
    pub sender: UnboundedSender<AsyncDeliveryTask>,
    pub pending: std::sync::Arc<AtomicUsize>,
    pub bounded_acp_queue: bool,
    pub acp_state: Option<AcpWorkerReadinessState>,
    pub acp_output_view: Option<Arc<dyn OutputView>>,
}

pub(super) fn build_worker_key(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> AsyncWorkerKey {
    AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle_name.to_string(),
        target_session: target_session.to_string(),
    }
}

static ASYNC_DELIVERY_REGISTRY: OnceLock<AsyncDeliveryRegistry> = OnceLock::new();

pub(super) fn async_delivery_registry() -> &'static AsyncDeliveryRegistry {
    ASYNC_DELIVERY_REGISTRY.get_or_init(AsyncDeliveryRegistry::default)
}

pub(super) fn async_worker_count() -> usize {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| workers.len())
        .unwrap_or(0)
}

pub(super) fn worker_exists(key: &AsyncWorkerKey) -> Result<bool, RelayError> {
    let workers = async_delivery_registry().workers.lock().map_err(|_| {
        super::super::relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    Ok(workers.contains_key(key))
}

pub(super) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    if !shutdown_requested() {
        return 0;
    }
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = async_worker_count();
        if remaining == 0 || Instant::now() >= deadline {
            return remaining;
        }
        thread::sleep(Duration::from_millis(ASYNC_SHUTDOWN_WAIT_POLL_MS));
    }
}

pub(super) fn try_existing_worker(
    key: &AsyncWorkerKey,
    task: AsyncDeliveryTask,
) -> Result<Option<AsyncDeliveryTask>, RelayError> {
    let registry = async_delivery_registry();
    let mut workers = registry.workers.lock().map_err(|_| {
        super::super::relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;

    if let Some(worker) = workers.get(key) {
        if worker.bounded_acp_queue && !reserve_acp_pending_slot(worker.pending.as_ref()) {
            return Err(super::super::relay_error(
                ACP_ERROR_CODE_QUEUE_FULL,
                "ACP worker queue is full",
                Some(json!({
                    "target_session": task.target_session,
                    "max_pending": ACP_MAX_PENDING,
                })),
            ));
        }
        match worker.sender.send(task) {
            Ok(()) => return Ok(None),
            Err(SendError(returned)) => {
                if worker.bounded_acp_queue {
                    release_pending_slot(worker.pending.as_ref());
                }
                workers.remove(key);
                return Ok(Some(returned));
            }
        }
    }
    Ok(Some(task))
}

pub(super) fn register_worker(
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: std::sync::Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock() {
        workers.insert(
            key,
            AsyncWorkerEntry {
                sender,
                pending,
                bounded_acp_queue,
                acp_state: None,
                acp_output_view: None,
            },
        );
    }
}

pub(super) fn register_worker_if_absent(
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: std::sync::Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> Result<bool, RelayError> {
    let mut workers = async_delivery_registry().workers.lock().map_err(|_| {
        super::super::relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    if workers.contains_key(&key) {
        return Ok(false);
    }
    workers.insert(
        key,
        AsyncWorkerEntry {
            sender,
            pending,
            bounded_acp_queue,
            acp_state: None,
            acp_output_view: None,
        },
    );
    Ok(true)
}

pub(in crate::relay) fn set_acp_worker_state(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
    state: AcpWorkerReadinessState,
) {
    let key = build_worker_key(bundle_name, runtime_directory, target_session);
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(&key)
    {
        entry.acp_state = Some(state);
    }
    // Publish to any observers regardless of whether a worker entry was
    // present. Publishers are keyed identically and live independently of
    // worker registration so subscribers can observe pre-registration and
    // post-unregistration transitions.
    super::observability::publish_acp_worker_state(&key, state);
}

pub(in crate::relay) fn get_acp_worker_state(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<AcpWorkerReadinessState> {
    let key = build_worker_key(bundle_name, runtime_directory, target_session);
    async_delivery_registry()
        .workers
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.acp_state)
}

pub(in crate::relay) fn install_acp_worker_output_view(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
    output_view: Option<Arc<dyn OutputView>>,
) {
    let key = build_worker_key(bundle_name, runtime_directory, target_session);
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(&key)
    {
        entry.acp_output_view = output_view;
    }
}

pub(in crate::relay) fn get_acp_worker_output_view(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<Arc<dyn OutputView>> {
    let key = build_worker_key(bundle_name, runtime_directory, target_session);
    async_delivery_registry()
        .workers
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.acp_output_view.clone())
}

pub(in crate::relay) fn acp_session_ready_for_startup(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> bool {
    matches!(
        get_acp_worker_state(bundle_name, runtime_directory, target_session),
        Some(AcpWorkerReadinessState::Available)
    )
}

pub(super) fn unregister_worker(key: &AsyncWorkerKey) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock() {
        workers.remove(key);
    }
}

pub(super) fn task_uses_acp_transport(task: &AsyncDeliveryTask) -> Result<bool, RelayError> {
    Ok(task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session)
        .map(|member| matches!(member.target, TargetConfiguration::Acp(_)))
        .unwrap_or(false))
}

pub(super) fn reserve_acp_pending_slot(pending: &AtomicUsize) -> bool {
    let mut current = pending.load(Ordering::Relaxed);
    loop {
        if current >= ACP_MAX_PENDING {
            return false;
        }
        match pending.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

pub(super) fn release_pending_slot(pending: &AtomicUsize) {
    let mut current = pending.load(Ordering::Relaxed);
    while current > 0 {
        match pending.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

pub(super) fn drop_pending_async_tasks_on_shutdown(
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &AtomicUsize,
) {
    while let Ok(task) = receiver.try_recv() {
        complete_task_on_shutdown(&task);
        release_pending_slot(pending);
    }
}

pub(super) fn complete_task_on_shutdown(task: &AsyncDeliveryTask) {
    complete_task_outcome(
        task,
        Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            details: None,
        }),
    );
}

pub(super) fn complete_task_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<SendResult, RelayError>,
) {
    match outcome {
        Ok(result) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_bundle_name.as_str(),
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
                    "bundle_name": task.bundle.bundle_name,
                    "sender_session": task.sender.id,
                    "target_session": result.target_session,
                    "message_id": result.message_id,
                    "outcome": result.outcome,
                    "reason_code": result.reason_code,
                    "reason": result.reason,
                    "details": result.details,
                }),
            );
        }
        Err(error) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_bundle_name.as_str(),
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
                    "bundle_name": task.bundle.bundle_name,
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
}

/// Routes a `delivery_outcome` event for one task back to the sender within its
/// home bundle. Takes the sender/target identity as discrete strings rather than
/// the whole task so ACP completion closures — which hold only cloned per-task
/// fields, not a `&AsyncDeliveryTask` that lives long enough — can emit terminal
/// outcomes once the agent turn finishes.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_sender_delivery_outcome_event(
    target_bundle_name: &str,
    sender_bundle_name: &str,
    sender_session: &str,
    target_session: &str,
    message_id: &str,
    terminal_outcome: SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) {
    let (phase, outcome) = match terminal_outcome {
        SendOutcome::Delivered => ("delivered", Some("success")),
        SendOutcome::Timeout => ("failed", Some("timeout")),
        SendOutcome::DroppedOnShutdown => ("failed", Some("failed")),
        SendOutcome::Failed => ("failed", Some("failed")),
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
        target_session: canonical_session_id(target_session, target_bundle_name),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload: serde_json::Value::Object(payload),
    };
    // Route the sender's delivery-outcome event back to the sender within its
    // home bundle, which differs from the target's bundle for cross-bundle sends.
    let _ = send_event_to_registered_ui(sender_bundle_name, sender_session, &event);
}
