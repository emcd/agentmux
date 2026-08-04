use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::json;

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
use super::super::fence::{FenceInProgress, FenceResolution, FenceVerdict};
use super::super::guard::{BatchId, GuardTrigger, TransportGeneration};
use crate::runtime::inscriptions::emit_inscription;

const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;

/// What one in-flight collector task yields: the resolved transport outcome, or
/// `None` if the outcome future was dropped before resolving.
///
/// Deliberately carries no task and no outcome *interpretation*. The member's
/// identity lives in the worker's [`InflightMember`] table instead, so a
/// collector that panics does not take its member's identity with it — which is
/// what lets the guard resolve that member rather than strand it.
type InflightOutcome = Option<SingleDeliveryOutcome>;

/// The worker-side record for one in-flight member, held outside the collector
/// task that carries its write.
pub(super) struct InflightMember {
    pub(super) task: AsyncDeliveryTask,
    /// Whether a successful delivery should clear startup failures: `true` for
    /// coder transports, `false` for UI.
    pub(super) record_served: bool,
    /// When this member was authorized, which is what the execution watchdog
    /// measures against.
    ///
    /// Anchored at authorization rather than at the write, because the bound is
    /// over the relay's own supervised execution and that execution begins the
    /// moment responsibility transfers. It never measures how long a target took
    /// to become ready — all of that waiting happens before authorization, on
    /// the `Pending` side, where it is deliberately unbounded.
    pub(super) authorized_at: Instant,
}

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
    // Member identities for the in-flight writes, keyed by their collector task's
    // id. Held here rather than inside the collector so a panicked collector
    // still leaves the relay able to name — and therefore resolve — its member.
    let mut inflight_members: HashMap<tokio::task::Id, InflightMember> = HashMap::new();
    let mut senders_dropped = false;
    // The post-authorization execution watchdog and the fence it initiates. The
    // bounds are the relay's own `[delivery]` settings, read once per worker.
    let delivery = super::super::admission::delivery_configuration();
    let submission_timeout = Duration::from_millis(delivery.submission_timeout_ms);
    let fence_observation = Duration::from_millis(delivery.fence_observation_timeout_ms);
    let mut fence: Option<FenceInProgress> = None;

    loop {
        if shutdown_requested() {
            // Close this worker to new sends BEFORE draining. Another worker
            // resolving a non-delivered outcome during its own shutdown routes a
            // terminal-outcome receipt here via `try_existing_worker`; once the
            // entry is marked closing, such a send bounces and is dropped
            // best-effort rather than being accepted into a receiver we will no
            // longer poll (an accept-after-drain race that would silently lose the
            // receipt and its terminal inscription). The entry stays registered so
            // the shutdown-barrier worker count still counts this still-draining
            // worker; the final unregister below drops it. Anything already queued
            // before this point is still drained.
            super::super::async_worker::close_worker(&key);
            shutdown_drain(
                transport.as_mut(),
                &mut inflight,
                &mut inflight_members,
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
        // The watchdog runs before the select so an elapsed member is noticed on
        // the same tick that observes it, and the fence advances even while the
        // worker is otherwise idle.
        if let Some(transport) = transport.as_mut() {
            advance_execution_watchdog(
                &key,
                transport,
                &mut fence,
                &mut inflight,
                &mut inflight_members,
                pending.as_ref(),
                submission_timeout,
                fence_observation,
            );
        }
        tokio::select! {
            // A fenced generation accepts no new submissions: the whole point of
            // the fence is that this generation stops producing effects.
            maybe_task = receiver.recv(), if !senders_dropped && fence.is_none() => {
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
                            &mut inflight_members,
                            pending.as_ref(),
                        );
                    }
                    None => senders_dropped = true,
                }
            }
            joined = inflight.join_next_with_id(), if !inflight.is_empty() => {
                if let Some(joined) = joined {
                    collect_outcome(joined, &mut inflight_members, pending.as_ref());
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
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
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
        // Authorization is the linearization point, so it precedes the write and
        // nothing else. Handing the envelope to the transport is recorded
        // separately, because the gap between the two is exactly the window in
        // which the guard can still prove nothing was written.
        authorize_member(&task);
        super::super::admission::note_handed_to_transport(task.message_id.as_str());
        (transport.mailw(build_ui_envelope(&task)), false)
    } else {
        authorize_member(&task);
        match prepare_coder_write(&task, transport) {
            Ok(future) => (future, true),
            Err(error) => {
                // Refused before any target-side effect: the member was never
                // handed over, so the guard's evidence order can prove
                // `not_submitted` rather than inferring the weaker unknown.
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return;
            }
        }
    };

    let handle = inflight.spawn(async move { future.await.ok() });
    inflight_members.insert(
        handle.id(),
        InflightMember {
            task,
            record_served,
            authorized_at: Instant::now(),
        },
    );
}

/// Drives the post-authorization execution watchdog and, once it has fired, the
/// generation fence it initiated.
///
/// The bound is an **execution watchdog over the relay's own supervised code**.
/// It states that our execution overran the time we allow it — not that the
/// target failed. That is what makes it categorically different from the absence
/// timers this change retires, which inferred target failure from an unchanged
/// screen. Every other trigger the guard has is an event; an executor that stays
/// alive and blocked produces none of them, so without this its quota would leak
/// and its target's ordering position would be held forever.
#[allow(clippy::too_many_arguments)]
fn advance_execution_watchdog(
    key: &AsyncWorkerKey,
    transport: &mut TransportImpl,
    fence: &mut Option<FenceInProgress>,
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    pending: &std::sync::atomic::AtomicUsize,
    submission_timeout: Duration,
    fence_observation: Duration,
) {
    let now = Instant::now();
    let Some(in_progress) = fence.as_mut() else {
        let overrun = inflight_members
            .values()
            .any(|member| now.duration_since(member.authorized_at) >= submission_timeout);
        if overrun {
            emit_inscription(
                "relay.delivery.watchdog.elapsed",
                &json!({
                    "namespace": key.namespace,
                    "target_session": key.target_session,
                    "submission_timeout_ms": submission_timeout.as_millis() as u64,
                    "unresolved_members": inflight_members.len(),
                }),
            );
            *fence = Some(FenceInProgress::begin(transport, fence_observation));
        }
        return;
    };

    // Nothing is terminalized at the bound itself: unit evidence stays
    // admissible through both fence windows, and the collect arm keeps running
    // alongside this. Only the verdict is the resolution cut.
    let Some(outcome) = in_progress.advance(transport, now) else {
        return;
    };
    *fence = None;

    emit_inscription(
        "relay.delivery.fence.verdict",
        &json!({
            "namespace": key.namespace,
            "target_session": key.target_session,
            "verdict": match outcome.verdict {
                FenceVerdict::Positive => "positive",
                FenceVerdict::Negative => "negative",
            },
            "resolution": match outcome.resolution {
                FenceResolution::Cooperative => "cooperative",
                FenceResolution::Forced => "forced",
                FenceResolution::Unobserved => "unobserved",
            },
            "unresolved_members": inflight_members.len(),
        }),
    );

    // The single resolution cut. Every member still unresolved terminalizes
    // through the guard's evidence order, from either verdict — a negative fence
    // withholds the target's replacement, never a member's outcome. A collector
    // resolving concurrently races this, and the guard's compare-and-swap is
    // what makes exactly one of them report.
    inflight.abort_all();
    for (_, member) in inflight_members.drain() {
        super::super::async_worker::complete_task_outcome_from_trigger(
            &member.task,
            GuardTrigger::FenceVerdict,
        );
        super::super::async_worker::release_pending_slot(pending);
    }

    if outcome.verdict == FenceVerdict::Negative {
        // Fail-stop. A target that admits no new generation is recoverable by
        // operator action; one whose old generation might still be writing while
        // a replacement runs is not.
        super::super::async_worker::mark_generation_fence_negative(key);
    }
}

/// Performs the `Pending` to `Authorized` transition for one member, minting its
/// batch and guard identities.
///
/// One member per batch today: the worker submits tasks one at a time, so a
/// batch is a batch of one until byte-budgeted round-robin lands and starts
/// forming real ones. The transport generation is likewise the worker's own,
/// which does not yet advance — generation replacement arrives with the fence,
/// and the guard key carries the component now so that work has somewhere to put
/// it rather than reshaping every key.
fn authorize_member(task: &AsyncDeliveryTask) {
    super::super::admission::authorize(
        task.message_id.as_str(),
        BatchId::mint(),
        TransportGeneration::default(),
    );
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
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    if let Some(transport) = transport {
        transport.shutdown();
    }
    while let Some(joined) = inflight.join_next_with_id().await {
        collect_outcome(joined, inflight_members, pending);
    }
    // Any member still in the table was never joined — its collector neither
    // resolved nor panicked, so the drain left it unresolved. Terminalize it
    // through the guard rather than dropping it: shutdown is a trigger like any
    // other, and the evidence order still knows whether it reached a transport.
    for (_, member) in inflight_members.drain() {
        super::super::async_worker::complete_task_outcome_from_trigger(
            &member.task,
            GuardTrigger::GracefulShutdown,
        );
    }
    super::super::async_worker::drop_pending_async_tasks_on_shutdown(receiver, pending);
}
