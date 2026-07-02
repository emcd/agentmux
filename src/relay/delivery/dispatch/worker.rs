use std::{sync::OnceLock, time::Duration};

use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::{
    configuration::{BundleMember, SessionType},
    envelope::PromptBatchSettings,
    runtime::signals::shutdown_requested,
};

use super::super::super::{AsyncDeliveryTask, DeliveryPayloadMode, RelayError};
use super::envelope::{
    build_acp_driver_services, build_coder_envelope, build_ui_envelope, build_worker_transport,
};
use super::outcomes::{collect_outcome, now_rfc3339};
use super::payload::{
    build_delivery_message, emit_envelope_metadata_inscription, resolve_target_member,
};

use crate::transports::{OutcomeFuture, SingleDeliveryOutcome, TransportImpl};

use super::super::async_worker::AsyncWorkerKey;

const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;

/// One in-flight write awaiting its transport [`OutcomeFuture`]. Carries the
/// originating task and whether a successful delivery should clear startup
/// failures (`true` for coder transports, `false` for UI), so the collect site
/// can map the resolved outcome onto a `SendResult` and complete the task. The
/// outcome is `None` if the future was dropped before resolving.
type InflightOutcome = (AsyncDeliveryTask, bool, Option<SingleDeliveryOutcome>);

#[derive(Clone)]
pub(super) struct AcpWorkerBootstrap {
    pub(super) target_member: BundleMember,
    pub(super) runtime_directory: std::path::PathBuf,
    /// Per-bundle choice-queue bound, captured into the chooser closure at worker
    /// construction so it no longer rides every delivery task and choice.
    pub(super) choices_pending_max: usize,
}

/// Spawns the per-target async delivery worker as a tokio task.
///
/// The worker runs a concurrent produce-and-collect loop: a `select!` over
/// `receiver.recv()` and a `JoinSet` of in-flight `OutcomeFuture`s submits
/// each task to its transport via the non-blocking `mailw`/`raww` seam and
/// collects the resolved outcomes. Blocking IO, quiescence/coalesce waits,
/// ACP bootstrap/respawn, and readiness mirroring all live inside the
/// transports' internal delivery tasks. Shutdown is observed via
/// `shutdown_requested()` polled between receives.
pub(super) fn spawn_async_delivery_worker(
    key: AsyncWorkerKey,
    receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    delivery_runtime_handle().spawn(async move {
        run_async_delivery_worker(key, receiver, pending, bootstrap).await;
    });
}

/// Resolves the tokio runtime handle that hosts delivery worker tasks.
///
/// In production the relay binary runs under `#[tokio::main]`, so a current
/// runtime handle is always available and we reuse it. In CLI/test contexts
/// where workers are enqueued without an ambient runtime (one-shot
/// `request_relay` callers, startup helpers driven directly from sync
/// tests), a process-wide fallback multi-thread runtime is created on
/// demand. The fallback is multi-thread with a blocking pool because the
/// transports' internal delivery tasks submit blocking IO via
/// `spawn_blocking`.
fn delivery_runtime_handle() -> Handle {
    if let Ok(handle) = Handle::try_current() {
        return handle;
    }
    static DELIVERY_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    DELIVERY_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("agentmux-delivery")
                .build()
                .expect("build agentmux delivery fallback runtime")
        })
        .handle()
        .clone()
}

async fn run_async_delivery_worker(
    key: AsyncWorkerKey,
    mut receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bootstrap: Option<AcpWorkerBootstrap>,
) {
    // Hold one `TransportImpl` for this target's lifetime. The transport KIND is
    // the only target-type-dependent decision, and it is fixed at construction
    // from the configured `session_type()` (transport-abstraction spec): ACP
    // targets get the bootstrap driver here; every other target's transport is
    // built lazily from its first task (a non-ACP worker has no bundle member at
    // spawn time — the task carries it) and then latched. Delivery is uniform:
    // the loop submits `mailw`/`raww` for every target with no registry-based
    // re-routing and no transport-deliverability gate. ACP lifecycle, readiness
    // mirroring, and respawn live entirely in the driver and its internal task;
    // the loop never names an ACP type.
    //
    // The loop is a concurrent produce-and-collect: it submits each task to the
    // transport via the non-blocking `mailw`/`raww` seam and concurrently collects
    // the resolved `OutcomeFuture`s. Coalescing, quiescence, the token-budget
    // combine, and the blocking IO all live inside each transport's internal
    // delivery task now, so the worker no longer batches, hoists quiescence, or
    // owns `spawn_blocking`.
    let batch_settings = super::prompt_batch_settings();
    // `None` until the transport is constructed: eagerly for ACP (bootstrap),
    // lazily from the first task's `session_type()` for every other target.
    let mut transport: Option<TransportImpl> = match bootstrap {
        Some(bootstrap) => {
            let services = build_acp_driver_services(&key, &bootstrap);
            let mut transport = TransportImpl::acp(
                bootstrap.target_member,
                bootstrap.runtime_directory,
                key.namespace.clone(),
                services,
                batch_settings,
            );
            if let TransportImpl::Acp(driver) = &mut transport {
                driver.bootstrap().await;
            }
            Some(transport)
        }
        None => None,
    };
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    // In-flight writes: each entry awaits one transport `OutcomeFuture` and yields
    // its originating task so the collect arm can complete it. Completion order is
    // independent of submission order; FIFO ordering at the target is preserved by
    // the transport's internal channel, into which the produce arm enqueues in
    // receive order.
    let mut inflight: JoinSet<InflightOutcome> = JoinSet::new();
    let mut senders_dropped = false;

    loop {
        if shutdown_requested() {
            shutdown_drain(
                transport.as_mut(),
                &mut inflight,
                &mut receiver,
                pending.as_ref(),
            )
            .await;
            break;
        }
        if senders_dropped && inflight.is_empty() {
            // No more producers and nothing in flight: the worker is unreachable.
            break;
        }
        tokio::select! {
            maybe_task = receiver.recv(), if !senders_dropped => {
                match maybe_task {
                    Some(task) => {
                        if shutdown_requested() {
                            super::super::async_worker::complete_task_on_shutdown(&task);
                            super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        submit_task(
                            task,
                            &key,
                            &mut transport,
                            batch_settings,
                            &mut inflight,
                            pending.as_ref(),
                        );
                    }
                    None => senders_dropped = true,
                }
            }
            joined = inflight.join_next(), if !inflight.is_empty() => {
                if let Some(joined) = joined {
                    collect_outcome(joined, pending.as_ref());
                }
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Poll tick: re-evaluate the shutdown gate even while idle.
            }
        }
    }
    super::super::async_worker::unregister_worker(&key);
}

/// Submits one task to its transport via the non-blocking write seam and spawns
/// an in-flight collector for its outcome. On the worker's first task the
/// transport is constructed from the target's configured `session_type()` and
/// latched (`build_worker_transport`). Delivery is then uniform: `Ui` builds the
/// stream envelope, coder transports (ACP/Tmux) render the framed envelope or
/// submit raw input, and the forward-declared `Pubsub` stub yields an explicit
/// not-implemented outcome (it is not deliverable). A construction or render
/// failure completes the task immediately and releases its slot.
fn submit_task(
    task: AsyncDeliveryTask,
    key: &AsyncWorkerKey,
    transport: &mut Option<TransportImpl>,
    batch_settings: PromptBatchSettings,
    inflight: &mut JoinSet<InflightOutcome>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if transport.is_none() {
        match build_worker_transport(&task, key, batch_settings) {
            Ok(built) => *transport = Some(built),
            Err(error) => {
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return;
            }
        }
    }
    let transport = transport
        .as_mut()
        .expect("worker transport constructed above");

    let (future, record_served) = if matches!(transport, TransportImpl::Pubsub) {
        // Forward-declared stub: not deliverable. Its `mailw`/`raww` are
        // `unimplemented!`, so produce an explicit terminal outcome instead of
        // calling them.
        super::super::async_worker::complete_task_outcome(
            &task,
            Err(super::super::super::session_type_not_implemented(
                task.target_session.as_str(),
                SessionType::Pubsub,
            )),
        );
        super::super::async_worker::release_pending_slot(pending);
        return;
    } else if matches!(transport, TransportImpl::Ui(_)) {
        (transport.mailw(build_ui_envelope(&task)), false)
    } else {
        match prepare_coder_write(&task, transport) {
            Ok(future) => (future, true),
            Err(error) => {
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return;
            }
        }
    };

    inflight.spawn(async move { (task, record_served, future.await.ok()) });
}

/// Builds a coder task's structured payload and submits it via the non-blocking
/// write seam. Envelope-mode tasks build a [`DeliveryMessage`] (and emit the
/// out-of-band metadata inscription) then go through `mailw`, where the transport
/// renders its own pane envelope; raw-input tasks go through `raww` with the
/// task's `append_enter`.
fn prepare_coder_write(
    task: &AsyncDeliveryTask,
    transport: &mut TransportImpl,
) -> Result<OutcomeFuture, RelayError> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let target_member = resolve_target_member(task)?;
            let message = build_delivery_message(task, target_member, now_rfc3339().as_str());
            emit_envelope_metadata_inscription(&message, task.message_id.as_str());
            Ok(transport.mailw(build_coder_envelope(task, message)))
        }
        DeliveryPayloadMode::RawInput => {
            Ok(transport.raww(task.message.clone(), task.append_enter))
        }
    }
}

/// Drains the worker on relay shutdown: signals the transport so its internal
/// delivery task resolves every in-flight write terminally, collects those
/// resolutions to completion, then drops the not-yet-submitted queued tasks. The
/// transport contract guarantees prompt terminal resolution on shutdown, so the
/// `join_next` drain does not park indefinitely. The transport is `None` if no
/// task ever arrived to construct it.
async fn shutdown_drain(
    transport: Option<&mut TransportImpl>,
    inflight: &mut JoinSet<InflightOutcome>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if let Some(transport) = transport {
        transport.shutdown();
    }
    while let Some(joined) = inflight.join_next().await {
        collect_outcome(joined, pending);
    }
    super::super::async_worker::drop_pending_async_tasks_on_shutdown(receiver, pending);
}
