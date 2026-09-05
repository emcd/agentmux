//! What a worker is constructed from, and how one generation is built.
//!
//! The transport *kind* is settled by configuration before any worker exists,
//! so it is resolved at the spawn site and reused for every generation this
//! worker goes on to build — which is what makes a replacement the same
//! construction as the original rather than a second path that has to agree
//! with it.

use std::sync::OnceLock;

use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver};

use crate::protocol::DeliveryDoorbell;
use crate::protocol::identity::{ConsumerBinding, ConsumerGenerationId, DeliveryTargetId};
use crate::relay::delivery::LedgerMailboxConsumer;
use crate::relay::delivery::admission::{
    GenerationRejection, claim_consumer_generation, register_doorbell, replace_consumer_generation,
};
use crate::relay::delivery::async_worker::report_resolved_member;
use crate::relay::delivery::fence::FenceVerdict;
use crate::relay::{AsyncDeliveryTask, RelayError, relay_error};
use crate::transports::{DeliveryExecutorContext, TransportImpl};
use crate::{configuration::BundleMember, envelope::PromptBatchSettings};

use super::super::super::async_worker::{
    AsyncWorkerKey, WorkerOwner, bind_worker_consumer_generation,
};
use super::super::envelope::{build_acp_driver_services, build_worker_transport};
use super::super::payload::resolve_target_member;
use super::run::run_async_delivery_worker;

pub(super) const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
/// Held back from the shutdown fence for the work that follows its verdict:
/// terminalizing members the collectors did not resolve, unregistering the
/// worker, and returning to a caller that still has its own teardown to run. A
/// fence that spent the entire remaining grace would satisfy its own bound and
/// leave every one of those to be cut off by the forced exit.
pub(super) const SHUTDOWN_FENCE_RESERVE_MS: u64 = 300;

/// What a worker builds its transport from, fixed at spawn and reused for every
/// generation it goes on to build.
///
/// The transport *kind* is a property of the target, settled by configuration
/// before any message exists. Holding that decision here rather than reading it
/// off whichever task arrives first is what lets construction happen once at
/// spawn, and what makes a replacement generation the same construction as the
/// original rather than a second code path that has to agree with it.
pub(in crate::relay::delivery::dispatch) enum WorkerTransportSource {
    /// ACP targets, whose runtime comes from a driver-owned supervised bootstrap.
    Acp(AcpWorkerBootstrap),
    /// Every other target, from inputs the spawn site resolved.
    Direct(WorkerTransportContext),
}

/// The construction inputs for a non-ACP transport, resolved at the spawn site.
///
/// Every field was already in the spawn site's hands — it is holding the task
/// that elected it — so the lazy read this replaces was never buying information.
/// It only deferred a construction failure past the point where the sender could
/// still be told synchronously, and left the worker with two construction paths
/// where the target only ever justified one.
#[derive(Clone)]
pub(in crate::relay::delivery::dispatch) struct WorkerTransportContext {
    pub(in crate::relay::delivery::dispatch) namespace: String,
    pub(in crate::relay::delivery::dispatch) runtime_directory: std::path::PathBuf,
    pub(in crate::relay::delivery::dispatch) target_session: String,
    /// `None` only for a relay-wide (UI) target. A configured coder target whose
    /// member is missing fails resolution instead of arriving here as `None`,
    /// which is what lets the builder treat absence as "UI" without a second flag.
    pub(in crate::relay::delivery::dispatch) target_member: Option<BundleMember>,
}

impl WorkerTransportContext {
    /// Resolves a worker's construction inputs from the task electing its spawn.
    pub(in crate::relay::delivery::dispatch) fn resolve(
        task: &AsyncDeliveryTask,
    ) -> Result<Self, RelayError> {
        let target_member = resolve_target_member(task)?.cloned();
        Ok(Self {
            namespace: task.bundle.bundle_name.clone(),
            runtime_directory: task.runtime_directory.clone(),
            target_session: task.target_session.clone(),
            target_member,
        })
    }
}

#[derive(Clone)]
pub(in crate::relay::delivery::dispatch) struct AcpWorkerBootstrap {
    pub(in crate::relay::delivery::dispatch) target_member: BundleMember,
    pub(in crate::relay::delivery::dispatch) runtime_directory: std::path::PathBuf,
    /// Per-bundle choice-queue bound, captured into the chooser closure at worker
    /// construction so it no longer rides every delivery task and choice.
    pub(in crate::relay::delivery::dispatch) choices_pending_max: usize,
}

/// Spawns the per-target async delivery worker as a tokio task.
///
/// The worker runs a produce-only loop: it receives tasks, places each one's
/// payload in its target's mailbox, and supervises the generation. It writes
/// nothing and collects no outcome, because the target's transport owns the
/// executor that does both. Blocking IO, coalescing, ACP bootstrap/respawn, and
/// readiness mirroring all live inside the transports. Shutdown is observed via
/// `shutdown_requested()` polled between receives.
pub(in crate::relay::delivery::dispatch) fn spawn_async_delivery_worker(
    key: AsyncWorkerKey,
    owner: WorkerOwner,
    receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    source: WorkerTransportSource,
) {
    delivery_runtime_handle().spawn(async move {
        run_async_delivery_worker(key, owner, receiver, pending, source).await;
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

/// How this generation comes to own its target's mailbox.
///
/// The two are genuinely different acts and only the caller knows which applies:
/// a first generation takes a target nobody holds, while a replacement takes one
/// from an incumbent and may do so only behind a positive fence verdict for it.
/// Naming the outgoing generation is what keeps a verdict from being spent on
/// whichever generation happens to be active by the time the flip runs.
#[derive(Clone, Copy, Debug)]
pub(super) enum ConsumerGenerationStep {
    /// The worker's first generation, for a target no consumer holds.
    Claim,
    /// A replacement for `outgoing`, whose fence reached a positive verdict.
    Replace { outgoing: ConsumerGenerationId },
}

/// One generation, everything it owns, and the identity it consumes under.
pub(super) struct BuiltGeneration {
    pub(super) transport: TransportImpl,
    /// The generation this one consumes its target's mailbox under, which the
    /// worker names as outgoing when it builds the next.
    pub(super) binding: ConsumerBinding,
}

/// Builds one generation: the identity it consumes under, the doorbell the relay
/// rings for it, and the transport whose executor drives both.
///
/// One function for both the worker's first generation and every replacement it
/// builds after a positive fence verdict. They must be the same construction: a
/// replacement that differed from the original would be a second transport kind
/// for one target, which the transport-abstraction contract does not allow. The
/// generation and the doorbell are inside that sameness rather than beside it —
/// a replacement that forgot either would be a consumer with no entitlement, or
/// a target reachable only by the poll.
///
/// **The generation is issued before the transport is built**, because the
/// transport is what will consume under it: an executor spawned by a
/// construction that had not yet been given an identity would have nothing to
/// bind to. A construction that then fails leaves the generation issued and the
/// target held, which is the worker's failure path's business — it unregisters,
/// and the reap that rides the unregister is what gives the target up. That is
/// why the generation is recorded on the registry entry here rather than on the
/// worker's success path: the reap names what the entry carries, so an entry
/// that only learned of a generation once its transport was built could never
/// release one whose transport was not.
///
/// **The doorbell is registered before the transport too**, which reverses the
/// order this had while nothing waited on one. The executor is spawned inside
/// `startup`, so a registration made afterwards would leave a window in which an
/// entry admitted to a live executor rang nothing — recoverable, since the
/// executor polls, but recoverable is a worse answer than unrepresentable when
/// the cost of the other order is a stale registration for a target that never
/// got a transport. Nothing rings such a registration: an entry reaches a mailbox
/// only through a worker's own intake, and this worker is about to end.
pub(super) async fn build_generation(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
    source: &WorkerTransportSource,
    batch_settings: PromptBatchSettings,
    readiness_changed: &std::sync::Arc<tokio::sync::Notify>,
    step: ConsumerGenerationStep,
) -> Result<BuiltGeneration, RelayError> {
    let target = consumer_target(key);
    let generation = issue_consumer_generation(&target, step)?;
    // Recorded on the registry entry in the same breath as it is issued, because
    // the entry is what the reap reads to name this generation and the reap is
    // the only thing that gives a target up. Binding here rather than on the
    // worker's success path is what covers a construction that fails after this
    // point: the generation is already held, and the failure path's unregister
    // has to be able to name it or the target is held for the life of the
    // process. A replacement rebinds for the same reason — it issues a new
    // identifier, and an entry still naming the outgoing one would leave the
    // incoming generation just as unreleasable.
    bind_worker_consumer_generation(key, owner, generation);
    let binding = ConsumerBinding::new(target.clone(), generation);
    let doorbell = register_generation_doorbell(&target);
    let delivery = executor_context(&binding, doorbell);
    let transport =
        build_transport(key, source, batch_settings, readiness_changed, delivery).await?;
    Ok(BuiltGeneration { transport, binding })
}

/// What the relay injects so this generation's executor can consume its target.
///
/// Assembled here because this is the one place that holds all four parts at
/// once: the binding just issued, the doorbell just registered, and the two
/// `[delivery]` durations. The executor paces itself by the same poll interval
/// this worker uses, so a missed ring costs the same delay on both sides of the
/// mailbox rather than two unrelated ones.
fn executor_context(
    binding: &ConsumerBinding,
    doorbell: DeliveryDoorbell,
) -> DeliveryExecutorContext {
    let delivery = crate::relay::delivery::admission::delivery_configuration();
    DeliveryExecutorContext {
        consumer: LedgerMailboxConsumer::bind(binding.clone()),
        doorbell,
        poll_interval: std::time::Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS),
        unreachable_dwell: std::time::Duration::from_millis(delivery.unreachable_dwell_ms),
    }
}

/// Takes the target's mailbox for this generation, by whichever act the step
/// names.
///
/// A replacement resolves what the outgoing generation had declared and not
/// acknowledged, and those members are reported here rather than swallowed: each
/// still owes its sender a terminal outcome, and the ledger call that resolved
/// them deliberately emits none.
fn issue_consumer_generation(
    target: &DeliveryTargetId,
    step: ConsumerGenerationStep,
) -> Result<ConsumerGenerationId, RelayError> {
    match step {
        ConsumerGenerationStep::Claim => claim_consumer_generation(target)
            .map_err(|rejection| generation_failure(step, rejection)),
        ConsumerGenerationStep::Replace { outgoing } => {
            // Positive by construction: this arm is reached only from the
            // positive branch of a fence verdict the worker has already
            // observed, and the ledger refuses anything else.
            let replacement = replace_consumer_generation(target, outgoing, FenceVerdict::Positive)
                .map_err(|rejection| generation_failure(step, rejection))?;
            for member in &replacement.resolved {
                report_resolved_member(
                    member,
                    None,
                    Some("the delivery generation was replaced before this unit was acknowledged"),
                );
            }
            Ok(replacement.generation)
        }
    }
}

/// Turns a refused claim or replacement into a construction failure.
///
/// A refusal here is a relay defect rather than a target problem: the worker's
/// own registration is what guarantees at most one worker per target, and the
/// reap that rides an unregister is what gives a target up, so a target that is
/// still held when a fresh worker claims it means one of those did not happen.
/// Failing construction is the loud reading — the worker resolves what it was
/// elected to carry and unregisters, so the next send may elect a fresh one —
/// where taking the target anyway would put two consumers on one mailbox, which
/// is the single condition every generation check exists to exclude.
fn generation_failure(step: ConsumerGenerationStep, rejection: GenerationRejection) -> RelayError {
    relay_error(
        "internal_unexpected_failure",
        "the relay could not take this target's mailbox for a new delivery generation",
        Some(serde_json::json!({
            "step": format!("{step:?}"),
            "rejection": format!("{rejection:?}"),
        })),
    )
}

/// The target a generation consumes, in the neutral boundary's spelling.
fn consumer_target(key: &AsyncWorkerKey) -> DeliveryTargetId {
    DeliveryTargetId::new(
        key.namespace.as_str(),
        key.runtime_directory.as_path(),
        key.target_session.as_str(),
    )
}

/// Registers the doorbell the relay rings for this generation's target, and
/// returns the handle its executor waits on.
///
/// Built here rather than beside `readiness_changed`, which belongs to the
/// worker and outlives every generation it goes on to build. A doorbell belongs
/// to the generation that waits on it, so registering as the generation is built
/// is what ties the two together.
///
/// **This is the only thing that displaces a registration.** Neither the reap
/// nor the fenced replacement clears one, so a generation's doorbell is
/// superseded by its successor's rather than removed when its own generation
/// ends. That is deliberate and load-bearing: both of those run behind the
/// target they act on, so a successor can already have registered here by the
/// time either reaches the ledger, and a clear would take the successor's
/// registration — which nothing would replace, since this runs once per
/// generation.
///
/// What the relay is handed is a closure, not the handle itself, so the relay
/// never learns what it rings — the same opaque shape a transport's readiness
/// notifier already has, invoked for a new event rather than through new
/// machinery. What the closure rings is the neutral [`DeliveryDoorbell`] this
/// returns, which is what the generation's delivery-loop executor waits on.
///
/// Registered *before* the transport is built, because the transport spawns the
/// executor that waits on this. A registration made afterwards would leave a
/// window in which an entry admitted to a live executor rang nothing; the other
/// order leaves at worst a stale registration for a generation whose construction
/// failed, and nothing rings one of those, since an entry reaches a mailbox only
/// through a worker's own intake and that worker is about to end.
fn register_generation_doorbell(target: &DeliveryTargetId) -> DeliveryDoorbell {
    let doorbell = DeliveryDoorbell::new();
    let ring = doorbell.clone();
    register_doorbell(target, std::sync::Arc::new(move || ring.ring()));
    doorbell
}

/// The transport half of building a generation, separated so the identity and
/// the doorbell above are settled before anything can consume under them.
async fn build_transport(
    key: &AsyncWorkerKey,
    source: &WorkerTransportSource,
    batch_settings: PromptBatchSettings,
    readiness_changed: &std::sync::Arc<tokio::sync::Notify>,
    delivery: DeliveryExecutorContext,
) -> Result<TransportImpl, RelayError> {
    match source {
        WorkerTransportSource::Acp(bootstrap) => {
            let services = build_acp_driver_services(key, bootstrap);
            let mut transport = TransportImpl::acp(
                bootstrap.target_member.clone(),
                bootstrap.runtime_directory.clone(),
                key.namespace.clone(),
                services,
                batch_settings,
                delivery,
            );
            // The bootstrap runs supervised in the background; construction itself
            // cannot fail here, which is why only the Direct arm returns an error.
            // The executor is spawned by the bootstrap's own `startup`, once there
            // is a client for it to write through.
            if let TransportImpl::Acp(driver) = &mut transport {
                driver.start_bootstrap();
            }
            Ok(transport)
        }
        WorkerTransportSource::Direct(context) => {
            let notify = std::sync::Arc::clone(readiness_changed);
            let readiness_notifier: crate::tmux::ReadinessNotifier =
                std::sync::Arc::new(move || notify.notify_waiters());
            build_worker_transport(context, key, batch_settings, readiness_notifier, delivery).await
        }
    }
}
