//! What a worker is constructed from, and how one generation is built.
//!
//! The transport *kind* is settled by configuration before any worker exists,
//! so it is resolved at the spawn site and reused for every generation this
//! worker goes on to build — which is what makes a replacement the same
//! construction as the original rather than a second path that has to agree
//! with it.

use std::{sync::OnceLock, time::Instant};

use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver};

use crate::protocol::DeliveryDoorbell;
use crate::protocol::identity::{ConsumerBinding, ConsumerGenerationId, DeliveryTargetId};
use crate::relay::delivery::admission::{
    GenerationRejection, claim_consumer_generation, register_doorbell, replace_consumer_generation,
};
use crate::relay::delivery::async_worker::report_resolved_member;
use crate::relay::delivery::fence::FenceVerdict;
use crate::relay::{AsyncDeliveryTask, RelayError, relay_error};
use crate::transports::{SingleDeliveryOutcome, TransportImpl};
use crate::{configuration::BundleMember, envelope::PromptBatchSettings};

use super::super::super::async_worker::{AsyncWorkerKey, WorkerOwner};
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

/// What one in-flight collector task yields: the resolved transport outcome, or
/// `None` if the outcome future was dropped before resolving.
///
/// Deliberately carries no task and no outcome *interpretation*. The member's
/// identity lives in the worker's [`InflightMember`] table instead, so a
/// collector that panics does not take its member's identity with it — which is
/// what lets the guard resolve that member rather than strand it.
pub(super) type InflightOutcome = Option<SingleDeliveryOutcome>;

/// The worker-side record for one in-flight member, held outside the collector
/// task that carries its write.
pub(in crate::relay::delivery::dispatch) struct InflightMember {
    pub(in crate::relay::delivery::dispatch) task: std::sync::Arc<AsyncDeliveryTask>,
    /// Whether a successful delivery should clear startup failures: `true` for
    /// coder transports, `false` for UI.
    pub(in crate::relay::delivery::dispatch) record_served: bool,
    /// When this member was authorized — the execution watchdog's anchor.
    ///
    /// Anchored at authorization rather than at the write because the bound covers
    /// the relay's whole supervised path, including the rendering and submission
    /// work between the two. Held here rather than in the ledger for the same
    /// reason the member's identity is: the worker is what can act on the bound,
    /// and it can only act on members it still holds.
    pub(in crate::relay::delivery::dispatch) authorized_at: Instant,
}

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
/// The worker runs a concurrent produce-and-collect loop: a `select!` over
/// `receiver.recv()` and a `JoinSet` of in-flight `OutcomeFuture`s submits
/// each task to its transport via the non-blocking `mailw`/`raww` seam and
/// collects the resolved outcomes. Blocking IO, quiescence/coalesce waits,
/// ACP bootstrap/respawn, and readiness mirroring all live inside the
/// transports' internal delivery tasks. Shutdown is observed via
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
    /// When an ACP bootstrap has settled; `None` for every other kind, which has
    /// no such wait.
    pub(super) bootstrap_settled: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The generation this one consumes its target's mailbox under, which the
    /// worker names as outgoing when it builds the next.
    pub(super) binding: ConsumerBinding,
    /// The handle the relay rings for this generation, and the one its
    /// delivery-loop executor waits on.
    pub(super) doorbell: DeliveryDoorbell,
}

/// Builds one generation: the identity it consumes under, its transport, the
/// flag that says when an ACP bootstrap has settled, and the doorbell the relay
/// rings for it.
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
/// and the reap that rides the unregister is what gives the target up.
pub(super) async fn build_generation(
    key: &AsyncWorkerKey,
    source: &WorkerTransportSource,
    batch_settings: PromptBatchSettings,
    readiness_changed: &std::sync::Arc<tokio::sync::Notify>,
    step: ConsumerGenerationStep,
) -> Result<BuiltGeneration, RelayError> {
    let target = consumer_target(key);
    let generation = issue_consumer_generation(&target, step)?;
    let (transport, bootstrap_settled) =
        build_transport(key, source, batch_settings, readiness_changed).await?;
    let doorbell = register_generation_doorbell(&target);
    Ok(BuiltGeneration {
        transport,
        bootstrap_settled,
        binding: ConsumerBinding::new(target, generation),
        doorbell,
    })
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
/// Registered *after* the transport is built, so a construction that fails
/// leaves no registration behind for a generation that never existed. The window
/// this opens is one in which a ring is lost, and it costs nothing: an executor's
/// first act is to peek, so it finds whatever arrived during it without being
/// told.
fn register_generation_doorbell(target: &DeliveryTargetId) -> DeliveryDoorbell {
    let doorbell = DeliveryDoorbell::new();
    let ring = doorbell.clone();
    register_doorbell(target, std::sync::Arc::new(move || ring.ring()));
    doorbell
}

/// The transport half of building a generation, separated so the doorbell above
/// is registered for a generation that exists rather than for one whose
/// construction is about to fail.
async fn build_transport(
    key: &AsyncWorkerKey,
    source: &WorkerTransportSource,
    batch_settings: PromptBatchSettings,
    readiness_changed: &std::sync::Arc<tokio::sync::Notify>,
) -> Result<
    (
        TransportImpl,
        Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ),
    RelayError,
> {
    match source {
        WorkerTransportSource::Acp(bootstrap) => {
            let services = build_acp_driver_services(key, bootstrap);
            let mut transport = TransportImpl::acp(
                bootstrap.target_member.clone(),
                bootstrap.runtime_directory.clone(),
                key.namespace.clone(),
                services,
                batch_settings,
            );
            // The bootstrap runs supervised in the background; construction itself
            // cannot fail here, which is why only the Direct arm returns an error.
            let settled = match &mut transport {
                TransportImpl::Acp(driver) => Some(driver.start_bootstrap()),
                _ => None,
            };
            Ok((transport, settled))
        }
        WorkerTransportSource::Direct(context) => {
            let notify = std::sync::Arc::clone(readiness_changed);
            let readiness_notifier: crate::tmux::ReadinessNotifier =
                std::sync::Arc::new(move || notify.notify_waiters());
            let transport =
                build_worker_transport(context, key, batch_settings, readiness_notifier).await?;
            Ok((transport, None))
        }
    }
}
