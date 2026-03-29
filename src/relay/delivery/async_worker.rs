use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;

use crate::configuration::TargetConfiguration;
use crate::runtime::{inscriptions::emit_inscription, signals::shutdown_requested};

use super::super::{AsyncDeliveryTask, ChatOutcome, ChatResult, RelayError};

use std::path::PathBuf;

const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const ACP_ERROR_CODE_QUEUE_FULL: &str = "runtime_acp_queue_full";
const ACP_MAX_PENDING: usize = 64;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct AsyncWorkerKey {
    pub tmux_socket: PathBuf,
    pub bundle_name: String,
    pub target_session: String,
}

#[derive(Default)]
pub(super) struct AsyncDeliveryRegistry {
    pub workers: Mutex<HashMap<AsyncWorkerKey, AsyncWorkerEntry>>,
}

pub(super) struct AsyncWorkerEntry {
    pub sender: mpsc::Sender<AsyncDeliveryTask>,
    pub pending: std::sync::Arc<AtomicUsize>,
    pub bounded_acp_queue: bool,
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
            Err(mpsc::SendError(returned)) => {
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
    sender: mpsc::Sender<AsyncDeliveryTask>,
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
            },
        );
    }
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
    receiver: &mpsc::Receiver<AsyncDeliveryTask>,
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
        Ok(ChatResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: ChatOutcome::DroppedOnShutdown,
            reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            details: None,
        }),
    );
}

pub(super) fn complete_task_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<ChatResult, RelayError>,
) {
    if let Some(sender) = task.completion_sender.as_ref() {
        let _ = sender.send(outcome);
        return;
    }
    match outcome {
        Ok(result) => emit_inscription(
            "relay.chat.async.completed",
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
        ),
        Err(error) => emit_inscription(
            "relay.chat.async.completed",
            &json!({
                "bundle_name": task.bundle.bundle_name,
                "sender_session": task.sender.id,
                "target_session": task.target_session,
                "message_id": task.message_id,
                "outcome": ChatOutcome::Failed,
                "reason": error.message,
                "error_code": error.code,
            }),
        ),
    }
}
