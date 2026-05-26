use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc as tokio_mpsc;

use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::configuration::{BundleConfiguration, BundleMember, TargetConfiguration};

use super::super::super::{AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendResult};
use super::super::acp_delivery::PersistentAcpWorkerRuntime;
use super::super::acp_state::{ACP_LOOK_PRIME_TIMEOUT_MS, ACP_STARTUP_PRIME_TIMEOUT_MS};
use super::super::async_worker::{AcpWorkerReadinessState, get_acp_worker_state};
use super::payload::{
    PreparedBatchPayload, PreparedDeliveryPayload, prepare_batch_delivery_payload,
    prepare_delivery_payload, resolve_target_member,
};
use super::transport::{deliver_non_ui_target, deliver_non_ui_target_batch};
use super::worker::{AcpWorkerBootstrap, spawn_async_delivery_worker};

pub(in crate::relay) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    super::super::async_worker::wait_for_async_delivery_shutdown(timeout)
}

pub(in crate::relay) fn await_acp_worker_prime_for_look(
    bundle: &BundleConfiguration,
    target_member: &BundleMember,
    runtime_directory: &std::path::Path,
) -> Result<bool, RelayError> {
    if !matches!(target_member.target, TargetConfiguration::Acp(_)) {
        return Ok(false);
    }
    let key = super::super::async_worker::AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle.bundle_name.clone(),
        target_session: target_member.id.clone(),
    };
    if !super::super::async_worker::worker_exists(&key)? {
        return Ok(false);
    }
    let deadline = Instant::now() + Duration::from_millis(ACP_LOOK_PRIME_TIMEOUT_MS);
    loop {
        let readiness = get_acp_worker_state(
            bundle.bundle_name.as_str(),
            runtime_directory,
            target_member.id.as_str(),
        );
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
    let key = super::super::async_worker::AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle_name.to_string(),
        target_session: target_member.id.clone(),
    };
    if !super::super::async_worker::worker_exists(&key).map_err(|error| {
        (
            "internal_unexpected_failure".to_string(),
            "failed to query ACP worker registry".to_string(),
            Some(json!({
                "target_session": target_member.id,
                "cause": error.message,
            })),
        )
    })? {
        let (sender, receiver) = tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bootstrap = AcpWorkerBootstrap {
            target_member: target_member.clone(),
            runtime_directory: runtime_directory.to_path_buf(),
        };
        if super::super::async_worker::register_worker_if_absent(
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
    let deadline = Instant::now() + Duration::from_millis(ACP_STARTUP_PRIME_TIMEOUT_MS);
    loop {
        let readiness =
            get_acp_worker_state(bundle_name, runtime_directory, target_member.id.as_str());
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
            Some(AcpWorkerReadinessState::Initializing | AcpWorkerReadinessState::Recovering)
            | None => {
                if Instant::now() >= deadline {
                    return Err((
                        "runtime_startup_failed".to_string(),
                        "ACP worker did not become ready during startup".to_string(),
                        Some(json!({
                            "target_session": target_member.id,
                            "timeout_ms": ACP_STARTUP_PRIME_TIMEOUT_MS,
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
) -> Result<SendResult, RelayError> {
    let (sender, receiver) = mpsc::channel::<Result<SendResult, RelayError>>();
    task.completion_sender = Some(sender);
    enqueue_delivery_task(task)?;
    receiver.recv().map_err(|source| {
        super::super::super::relay_error(
            "internal_unexpected_failure",
            "failed to receive sync delivery result from worker",
            Some(json!({"cause": source.to_string()})),
        )
    })?
}

fn enqueue_delivery_task(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let bounded_acp_queue = super::super::async_worker::task_uses_acp_transport(&task)?;
    let key = super::super::async_worker::AsyncWorkerKey {
        runtime_directory: task.runtime_directory.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        target_session: task.target_session.clone(),
    };
    if bounded_acp_queue && !super::super::async_worker::worker_exists(&key)? {
        return Err(super::super::super::relay_error(
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": task.target_session,
            })),
        ));
    }
    match super::super::async_worker::try_existing_worker(&key, task)? {
        None => Ok(()),
        Some(task) => {
            if bounded_acp_queue {
                return Err(super::super::super::relay_error(
                    "runtime_acp_worker_unavailable",
                    "ACP worker is unavailable for target session",
                    Some(json!({
                        "target_session": task.target_session,
                    })),
                ));
            }
            let (sender, receiver) = tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            if bounded_acp_queue
                && !super::super::async_worker::reserve_acp_pending_slot(pending.as_ref())
            {
                return Err(super::super::super::relay_error(
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
                    super::super::async_worker::release_pending_slot(pending.as_ref());
                }
                super::super::super::relay_error(
                    "internal_unexpected_failure",
                    "failed to enqueue async delivery task",
                    Some(json!({"cause": source.to_string()})),
                )
            })?;
            spawn_async_delivery_worker(key.clone(), receiver, pending.clone(), None);
            super::super::async_worker::register_worker(key, sender, pending, bounded_acp_queue);
            Ok(())
        }
    }
}

pub(in crate::relay) fn deliver_one_target(
    task: &AsyncDeliveryTask,
) -> Result<SendResult, RelayError> {
    let mut acp_runtime = None;
    deliver_one_target_with_worker_state(task, &mut acp_runtime)
}

pub(in crate::relay) fn deliver_one_target_with_worker_state(
    task: &AsyncDeliveryTask,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> Result<SendResult, RelayError> {
    let created_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let target_member = resolve_target_member(task)?;
    let prepared_payload = prepare_delivery_payload(task, target_member, created_at.as_str())?;
    let prompt_batches = match prepared_payload {
        PreparedDeliveryPayload::Immediate(result) => return Ok(result),
        PreparedDeliveryPayload::Batched { prompt_batches } => prompt_batches,
    };

    let non_ui_target_member = target_member.expect("non-UI target_member must exist");
    deliver_non_ui_target(task, non_ui_target_member, prompt_batches, acp_runtime)
}

/// Delivers a coalesced batch of tasks against the shared target.
///
/// Returns one outcome per task in `batch`, aligned to its slice index, plus
/// the tail tasks that were peeled (ACP single-batch invariant) and must
/// return to the worker's carry buffer for the next iteration.
///
/// RawInput tasks never coalesce (the worker's `can_coalesce_with_head`
/// predicate refuses them), so a RawInput head implies `batch.len() == 1`
/// and is delegated to the single-task path verbatim — the existing tmux
/// raw-input semantics carry through unchanged.
///
/// `pre_resolved_pane` is supplied by the worker loop when it has already
/// performed the tmux quiescence wait so that post-quiescence task arrivals
/// could be drained into the batch. Threaded into `deliver_non_ui_target_batch`
/// where it short-circuits the in-transport pane resolution + wait.
pub(in crate::relay) fn deliver_batch_with_worker_state(
    batch: &[AsyncDeliveryTask],
    pre_resolved_pane: Option<String>,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> (Vec<Result<SendResult, RelayError>>, Vec<AsyncDeliveryTask>) {
    debug_assert!(
        !batch.is_empty(),
        "deliver_batch_with_worker_state requires a non-empty batch",
    );
    let head = &batch[0];

    // Raw-input head: never coalesces by predicate; fall through to the
    // single-task path so the existing tmux raw-input shape is preserved.
    if matches!(head.payload_mode, DeliveryPayloadMode::RawInput) {
        debug_assert_eq!(
            batch.len(),
            1,
            "RawInput batches must not coalesce; got {} tasks",
            batch.len(),
        );
        let outcome = deliver_one_target_with_worker_state(head, acp_runtime);
        return (vec![outcome], Vec::new());
    }

    let created_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let target_member = match resolve_target_member(head) {
        Ok(value) => value,
        Err(error) => return (fanned_errors(batch, error), Vec::new()),
    };

    let prepared = match prepare_batch_delivery_payload(batch, target_member, created_at.as_str()) {
        Ok(value) => value,
        Err(error) => return (fanned_errors(batch, error), Vec::new()),
    };

    match prepared {
        PreparedBatchPayload::Immediate(result) => {
            // UI head: batch.len() == 1 by construction (the coalesce
            // predicate refuses to coalesce UI-routed tasks).
            (vec![Ok(result)], Vec::new())
        }
        PreparedBatchPayload::Batched {
            prompt_batches,
            accepted_len,
        } => {
            debug_assert!(accepted_len >= 1);
            let nonui_target_member = target_member.expect("non-UI target_member must exist");
            let accepted = &batch[..accepted_len];
            let deferred = batch[accepted_len..].to_vec();
            let outcomes = deliver_non_ui_target_batch(
                accepted,
                nonui_target_member,
                prompt_batches,
                pre_resolved_pane,
                acp_runtime,
            );
            (outcomes, deferred)
        }
    }
}

// Fans a single `RelayError` out into one `Err` per task in `batch`, so the
// caller can call `complete_task_outcome` once per task without losing the
// shared cause when target resolution or envelope rendering fails before
// transport.
fn fanned_errors(
    batch: &[AsyncDeliveryTask],
    error: RelayError,
) -> Vec<Result<SendResult, RelayError>> {
    batch.iter().map(|_| Err(error.clone())).collect()
}
