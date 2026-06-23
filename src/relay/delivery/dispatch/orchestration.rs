use std::{
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc as tokio_mpsc;

use serde_json::json;

use crate::configuration::{BundleMember, TargetConfiguration};

use super::super::super::{AsyncDeliveryTask, RelayError};
use crate::acp::state::ACP_STARTUP_PRIME_TIMEOUT_MS;

use super::super::async_worker::get_acp_worker_state;
use super::worker::{AcpWorkerBootstrap, spawn_async_delivery_worker};
use crate::transports::AcpWorkerReadinessState;

pub(in crate::relay) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    super::super::async_worker::wait_for_async_delivery_shutdown(timeout)
}

pub(in crate::relay) fn initialize_acp_target_for_startup(
    bundle_name: &str,
    runtime_directory: &std::path::Path,
    target_member: &BundleMember,
    choices_pending_max: usize,
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
    let key = super::super::async_worker::build_worker_key(
        bundle_name,
        runtime_directory,
        target_member.id.as_str(),
    );
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
            choices_pending_max,
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

fn enqueue_delivery_task(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let bounded_acp_queue = super::super::async_worker::task_uses_acp_transport(&task);
    let key = super::super::async_worker::build_worker_key(
        task.bundle.bundle_name.as_str(),
        task.runtime_directory.as_path(),
        task.target_session.as_str(),
    );
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
            // ACP workers are pre-created during startup and never lazily created
            // here. Reaching the new-worker path with an ACP task means its worker
            // has gone away, so report it unavailable rather than spin up a
            // non-bootstrap worker. This guard makes the rest of this arm provably
            // non-ACP, so the worker created below is always unbounded.
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
            sender.send(task).map_err(|source| {
                super::super::super::relay_error(
                    "internal_unexpected_failure",
                    "failed to enqueue async delivery task",
                    Some(json!({"cause": source.to_string()})),
                )
            })?;
            spawn_async_delivery_worker(key.clone(), receiver, pending.clone(), None);
            super::super::async_worker::register_worker(key, sender, pending, false);
            Ok(())
        }
    }
}
