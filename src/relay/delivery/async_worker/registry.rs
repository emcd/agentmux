//! The async delivery worker registry: which worker owns a target, what state it
//! is in, and the accounting that bounds an ACP worker's queue.
//!
//! Registration is an *election*. A spawner installs an entry to claim a target,
//! and the entry's presence is what keeps a second generation from starting
//! alongside it. That is why several operations here are ownership-checked rather
//! than unconditional: a worker that lost a race must never remove the entry its
//! successor installed.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, error::SendError};

use crate::configuration::{BundleMember, TargetConfiguration};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::runtime::signals::shutdown_requested;
use crate::tmux::TmuxOutputView;
use crate::transports::{OutputView, WorkerFailureReason, WorkerReadinessState};

use crate::relay::delivery::observability;
use crate::relay::{AsyncDeliveryTask, RelayError, relay_error};

use super::terminal::{complete_task_on_shutdown, complete_task_outcome_from_trigger};
use crate::relay::delivery::guard::GuardTrigger;

use std::path::{Path, PathBuf};

const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const ACP_ERROR_CODE_QUEUE_FULL: &str = "runtime_acp_queue_full";
const ACP_PENDING_MAX: usize = 64;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(in crate::relay::delivery) struct AsyncWorkerKey {
    pub runtime_directory: PathBuf,
    pub namespace: String,
    pub target_session: String,
}

#[derive(Default)]
pub(in crate::relay::delivery) struct AsyncDeliveryRegistry {
    pub workers: Mutex<HashMap<AsyncWorkerKey, AsyncWorkerEntry>>,
}

pub(in crate::relay::delivery) struct AsyncWorkerEntry {
    /// Identifies the worker that installed this entry.
    ///
    /// Registry keys are per-target and outlive individual workers, so a key
    /// alone cannot say *which* worker an entry belongs to. Without that, a
    /// worker exiting could remove a successor's entry and strand a live
    /// worker's only sender.
    pub owner: WorkerOwner,
    pub sender: UnboundedSender<AsyncDeliveryTask>,
    pub pending: std::sync::Arc<AtomicUsize>,
    pub bounded_acp_queue: bool,
    pub readiness: Option<WorkerReadinessState>,
    /// The worker's most recent unrecoverable failure, recorded just before its
    /// `Unavailable` transition and cleared once it returns to a healthy state.
    /// Lets the startup poller surface the true cause behind an `Unavailable`
    /// readiness rather than a generic placeholder.
    pub last_failure: Option<WorkerFailureReason>,
    pub acp_output_view: Option<Arc<dyn OutputView>>,
    /// Set once the worker begins its shutdown drain so new sends bounce rather
    /// than landing in a receiver that will no longer be polled. The entry stays
    /// registered (so the shutdown-barrier count still reflects a worker that is
    /// still draining) until the worker unregisters at the end of its run.
    pub closing: bool,
    /// Set when this worker's bundle is being torn down, asking it to end the way
    /// process shutdown ends it.
    ///
    /// Distinct from `closing`, which the worker sets on *itself* once it has
    /// begun draining. This is the inbound request, written by a lifecycle path
    /// that is not the worker, and the worker observes it on its own poll rather
    /// than being interrupted — the drain has to run on the worker's runtime, so
    /// the only sound signal is one it reads.
    pub stopping: bool,
    /// Set when this target's generation fence returned a negative verdict:
    /// cessation was not observed, so an old generation may still be able to write
    /// to the target.
    ///
    /// The entry is deliberately kept registered for the rest of the relay's life
    /// once this is set. Holding the key is *how* no replacement is admitted —
    /// every spawner elects itself by installing an entry, so an entry that never
    /// leaves means no second generation can start alongside one that might still
    /// be writing. Recovery is by operator action, which is the fail-stop the
    /// fence chooses over an ordering hazard.
    pub fail_stopped: bool,
}

pub(in crate::relay::delivery) fn build_worker_key(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> AsyncWorkerKey {
    AsyncWorkerKey {
        runtime_directory: runtime_directory.to_path_buf(),
        namespace: namespace.to_string(),
        target_session: target_session.to_string(),
    }
}

static ASYNC_DELIVERY_REGISTRY: OnceLock<AsyncDeliveryRegistry> = OnceLock::new();

pub(in crate::relay::delivery) fn async_delivery_registry() -> &'static AsyncDeliveryRegistry {
    ASYNC_DELIVERY_REGISTRY.get_or_init(AsyncDeliveryRegistry::default)
}

pub(in crate::relay::delivery) fn async_worker_count() -> usize {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| workers.len())
        .unwrap_or(0)
}

pub(in crate::relay::delivery) fn worker_exists(key: &AsyncWorkerKey) -> Result<bool, RelayError> {
    let workers = async_delivery_registry().workers.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    Ok(workers.contains_key(key))
}

pub(in crate::relay::delivery) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
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

/// Outcome of handing a task to an already-registered worker without spawning
/// one. Distinguishes the task landing in a live worker from the two no-op cases
/// the caller must treat differently: a *missing* worker (spawn one, or report
/// the ACP target unavailable) versus a *closing* worker draining for shutdown
/// (drop the task best-effort; never resurrect a worker mid-shutdown). Collapsing
/// the two into one "bounced" case lets a Send racing shutdown clobber a closing
/// entry the shutdown barrier still counts.
pub(in crate::relay::delivery) enum WorkerDispatch {
    /// The task was accepted by a live, non-closing worker.
    Accepted,
    /// No worker is registered for the key at all; the task is returned so the
    /// caller can spawn one or, for an ACP target, report it unavailable.
    ///
    /// Distinct from [`Dropped`](Self::Dropped) because the two mean opposite
    /// things to an observer even though they mean the same thing to a spawner.
    /// Nothing was ever there, so no path closed.
    Missing(AsyncDeliveryTask),
    /// A worker *was* registered and its receiver has since been dropped. The
    /// entry is removed here and the task returned, so a spawner treats this
    /// exactly like [`Missing`](Self::Missing).
    ///
    /// It is a separate variant because a notification path that existed and
    /// closed SHALL be counted and recorded, while one that never existed is an
    /// ordinary offline state. Collapsing them — as an earlier version of this
    /// enum did — makes that distinction unrepresentable at every call site.
    Dropped(AsyncDeliveryTask),
    /// The worker is draining for shutdown and will not poll its receiver again;
    /// the task is returned so the caller drops it best-effort.
    Closing(AsyncDeliveryTask),
    /// The target's generation fence returned a negative verdict, so the relay
    /// admits no further writes to it — including the replacement generation a
    /// spawner would otherwise start.
    ///
    /// Distinct from [`Closing`](Self::Closing) because the two are opposite
    /// claims. A closing worker is the relay stopping on purpose, and its members
    /// are honestly reported as dropped on shutdown; a fail-stopped one is the
    /// relay unable to establish that an old generation stopped, which is a
    /// condition the sender must be told about rather than have spelled as a
    /// shutdown.
    FailStopped(AsyncDeliveryTask),
}

/// Hands a task to an existing worker, and in doing so fixes its position in
/// that target's queue.
///
/// **This is the per-target FIFO's linearization point, and the order it
/// establishes is worker-enqueue order — not request order and not admission
/// order.** `sender.send` runs while the registry lock is held, so two senders
/// racing toward one target serialize here, and because the channel is unbounded
/// the send cannot block and channel order is therefore lock-acquisition order.
///
/// Admission is reserved earlier, in each request handler, so a request may
/// reserve its quota first and still lose this race. Nothing corrects for that,
/// and nothing should: admission answers whether the queue has room, while this
/// answers where in the queue the work lands. Documenting the weaker of the two
/// is deliberate, because it is the one the implementation provides and the one
/// a test can hold it to.
///
/// Mail and raw are one order rather than two because both reach this function
/// through `enqueue_async_delivery` with the same worker key. No handler
/// consults the other's queue, so this shared path is the whole of the
/// mechanism.
pub(in crate::relay::delivery) fn try_existing_worker(
    key: &AsyncWorkerKey,
    task: AsyncDeliveryTask,
) -> Result<WorkerDispatch, RelayError> {
    let registry = async_delivery_registry();
    let mut workers = registry.workers.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;

    if let Some(worker) = workers.get(key) {
        // Read before `closing`: a fail-stopped target is fail-stopped whether or
        // not the relay later starts shutting down, and reporting it as a
        // shutdown drop would spell an ordering hazard as an orderly stop.
        if worker.fail_stopped {
            return Ok(WorkerDispatch::FailStopped(task));
        }
        if worker.closing {
            // The worker is draining for shutdown and will not poll its receiver
            // again; bounce the task rather than accepting it into a dead queue.
            return Ok(WorkerDispatch::Closing(task));
        }
        if worker.bounded_acp_queue && !reserve_acp_pending_slot(worker.pending.as_ref()) {
            return Err(relay_error(
                ACP_ERROR_CODE_QUEUE_FULL,
                "ACP worker queue is full",
                Some(json!({
                    "target_session": task.target_session,
                    "pending_max": ACP_PENDING_MAX,
                })),
            ));
        }
        match worker.sender.send(task) {
            Ok(()) => return Ok(WorkerDispatch::Accepted),
            Err(SendError(returned)) => {
                if worker.bounded_acp_queue {
                    release_pending_slot(worker.pending.as_ref());
                }
                workers.remove(key);
                return Ok(WorkerDispatch::Dropped(returned));
            }
        }
    }
    Ok(WorkerDispatch::Missing(task))
}

/// Identifies one worker installation, distinct from every other for the same
/// target. Minted by whichever caller wins the registry insert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::relay::delivery) struct WorkerOwner(u64);

static NEXT_WORKER_OWNER: AtomicU64 = AtomicU64::new(1);

impl WorkerOwner {
    fn mint() -> Self {
        Self(NEXT_WORKER_OWNER.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
pub(in crate::relay::delivery) fn register_worker(
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: std::sync::Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> WorkerOwner {
    let owner = WorkerOwner::mint();
    if let Ok(mut workers) = async_delivery_registry().workers.lock() {
        workers.insert(
            key,
            AsyncWorkerEntry {
                owner,
                sender,
                pending,
                bounded_acp_queue,
                readiness: None,
                last_failure: None,
                acp_output_view: None,
                closing: false,
                stopping: false,
                fail_stopped: false,
            },
        );
    }
    owner
}

pub(in crate::relay::delivery) fn register_worker_if_absent(
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: std::sync::Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> Result<Option<WorkerOwner>, RelayError> {
    let mut workers = async_delivery_registry().workers.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    if workers.contains_key(&key) {
        return Ok(None);
    }
    let owner = WorkerOwner::mint();
    workers.insert(
        key,
        AsyncWorkerEntry {
            owner,
            sender,
            pending,
            bounded_acp_queue,
            readiness: None,
            last_failure: None,
            acp_output_view: None,
            closing: false,
            stopping: false,
            fail_stopped: false,
        },
    );
    Ok(Some(owner))
}

/// Marks a target fail-stopped after a negative generation-fence verdict, so
/// every further send to it is refused rather than queued or handed to a
/// replacement generation.
///
/// The entry is left in place afterwards and never removed by its worker. That is
/// the mechanism, not an oversight: registration is the election a spawner has to
/// win, so an entry that outlives its worker is exactly "admit no replacement for
/// this target". `raww` needs no separate barrier because it reaches the target
/// through this same registry lookup.
pub(in crate::relay::delivery) fn mark_worker_fail_stopped(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(key)
        && entry.owner == owner
    {
        entry.fail_stopped = true;
    }
}

/// Whether this worker has been asked to stop because its bundle is being torn
/// down. Read by the worker's own loop, which is the only thing that may act on
/// it: the drain runs on the worker's runtime.
pub(in crate::relay::delivery) fn worker_stop_requested(key: &AsyncWorkerKey) -> bool {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| workers.get(key).is_some_and(|entry| entry.stopping))
        .unwrap_or(false)
}

/// Asks every worker belonging to one bundle to stop, and reports how many were
/// signalled.
///
/// A bundle is identified here exactly as the worker key identifies it — by
/// namespace *and* runtime directory. Namespace alone would reach a same-named
/// bundle hosted by a different relay out of the same process.
///
/// Fail-stopped workers are deliberately skipped. Their entry is the whole of the
/// no-replacement guarantee: a negative fence verdict means an old generation may
/// still be able to write to that target, and a bundle stop is not evidence that
/// it stopped. Removing the entry so a reload could elect a replacement is exactly
/// the ordering hazard the fail-stop exists to refuse, so such a target stays
/// held until an operator resolves it.
///
/// Signalling only asks. The worker ends on its own next poll, which is what
/// [`wait_for_bundle_workers_stopped`] waits for.
pub(in crate::relay) fn stop_workers_for_bundle(
    namespace: &str,
    runtime_directory: &Path,
) -> usize {
    let Ok(mut workers) = async_delivery_registry().workers.lock() else {
        return 0;
    };
    let mut signalled = 0usize;
    for (key, entry) in workers.iter_mut() {
        if key.namespace != namespace || key.runtime_directory != runtime_directory {
            continue;
        }
        if entry.fail_stopped {
            continue;
        }
        entry.stopping = true;
        signalled += 1;
    }
    signalled
}

/// Waits for the workers of one bundle to finish draining and leave the registry,
/// returning how many were still present when the wait ended.
///
/// A non-zero return is the honest report that teardown was not observed to
/// complete, not a failure to act: the stop has been requested and the drain is
/// bounded by the same fence every other generation ending uses. Fail-stopped
/// entries are excluded for the reason [`stop_workers_for_bundle`] gives — they
/// never leave, so counting them would turn every wait into a full timeout.
pub(in crate::relay) fn wait_for_bundle_workers_stopped(
    namespace: &str,
    runtime_directory: &Path,
    timeout: Duration,
) -> usize {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = bundle_worker_count(namespace, runtime_directory);
        if remaining == 0 || Instant::now() >= deadline {
            return remaining;
        }
        thread::sleep(Duration::from_millis(ASYNC_SHUTDOWN_WAIT_POLL_MS));
    }
}

/// Registered, non-fail-stopped workers belonging to one bundle.
fn bundle_worker_count(namespace: &str, runtime_directory: &Path) -> usize {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| {
            workers
                .iter()
                .filter(|(key, entry)| {
                    key.namespace == namespace
                        && key.runtime_directory == runtime_directory
                        && !entry.fail_stopped
                })
                .count()
        })
        .unwrap_or(0)
}

/// Marks a worker as closing so it bounces new sends while its entry stays
/// registered, preserving the shutdown-barrier worker count until the worker
/// finishes draining and unregisters. Called at the start of the shutdown drain
/// to close the accept-after-drain race without dropping the count early.
pub(in crate::relay::delivery) fn close_worker(key: &AsyncWorkerKey, owner: WorkerOwner) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(key)
        && entry.owner == owner
    {
        entry.closing = true;
    }
}

pub(in crate::relay) fn set_worker_readiness(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
    state: WorkerReadinessState,
) {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(&key)
    {
        entry.readiness = Some(state);
        // A worker that reaches a healthy state has recovered; drop any stale
        // failure so a later `Unavailable` is never attributed to an old cause.
        if matches!(
            state,
            WorkerReadinessState::Available | WorkerReadinessState::Busy
        ) {
            entry.last_failure = None;
        }
    }
    // Publish to any observers regardless of whether a worker entry was
    // present. Publishers are keyed identically and live independently of
    // worker registration so subscribers can observe pre-registration and
    // post-unregistration transitions.
    observability::publish_worker_readiness(&key, state);
}

pub(in crate::relay) fn get_worker_readiness(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<WorkerReadinessState> {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    async_delivery_registry()
        .workers
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.readiness)
}

pub(in crate::relay) fn set_worker_failure(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
    failure: WorkerFailureReason,
) {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(&key)
    {
        entry.last_failure = Some(failure);
    }
}

pub(in crate::relay) fn get_worker_failure(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<WorkerFailureReason> {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    async_delivery_registry()
        .workers
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.last_failure.clone())
}

pub(in crate::relay) fn install_acp_worker_output_view(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
    output_view: Option<Arc<dyn OutputView>>,
) {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(&key)
    {
        entry.acp_output_view = output_view;
    }
}

pub(in crate::relay) fn get_acp_worker_output_view(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<Arc<dyn OutputView>> {
    let key = build_worker_key(namespace, runtime_directory, target_session);
    async_delivery_registry()
        .workers
        .lock()
        .ok()?
        .get(&key)
        .and_then(|entry| entry.acp_output_view.clone())
}

/// Resolves the polymorphic [`OutputView`] handle for a look target, hiding the
/// per-transport handle provenance from the relay look handler.
///
/// Provenance: a worker-published handle from the delivery registry when present
/// (ACP today, and any future worker-backed transport), otherwise a
/// config-constructed [`TmuxOutputView`] for tmux members whose output is
/// addressable directly through the socket. Returns `None` for non-lookable
/// session types and for an ACP target with no published handle (unstarted,
/// failed bootstrap, or mid-respawn); the look handler maps that `None` to an
/// empty stale/unavailable snapshot.
pub(in crate::relay) fn get_output_view(
    namespace: &str,
    runtime_directory: &Path,
    member: &BundleMember,
) -> Option<Arc<dyn OutputView>> {
    match &member.target {
        TargetConfiguration::Acp(_) => {
            get_acp_worker_output_view(namespace, runtime_directory, member.id.as_str())
        }
        TargetConfiguration::Tmux(_) => {
            let socket_path = tmux_socket_path_for_runtime_directory(runtime_directory);
            Some(Arc::new(TmuxOutputView::new(
                socket_path,
                member.id.clone(),
            )))
        }
        TargetConfiguration::Pty(_) => {
            // Pty sessions surface look output via the transport's own
            // PtyOutputView handle; the relay constructs the transport
            // lazily on first look request.
            None
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => None,
    }
}

/// Whether a worker readiness state counts as a ready ACP session.
///
/// The single acceptance set for "this session is ready": the startup poll waits
/// for it, and the `list` projection reports it. Two predicates here mean `up`
/// and `list` can disagree about the same session, which is exactly the
/// contradiction `degraded` was defined to avoid — it is specified as one
/// condition across both surfaces.
///
/// `Busy` is ready. A session mid-turn is serving, not down; excluding it would
/// report a bundle as degraded — or a single-member bundle as `down` — for the
/// whole duration of an agent's turn. The registry already treats
/// `Available | Busy` as the healthy pair when it clears a recorded failure.
pub(in crate::relay) fn acp_readiness_is_ready(readiness: Option<WorkerReadinessState>) -> bool {
    matches!(
        readiness,
        Some(WorkerReadinessState::Available | WorkerReadinessState::Busy)
    )
}

pub(in crate::relay) fn acp_session_is_ready(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> bool {
    acp_readiness_is_ready(get_worker_readiness(
        namespace,
        runtime_directory,
        target_session,
    ))
}

/// Removes this target's registry entry, but only if `owner` still holds it.
///
/// The ownership check is what keeps an exiting worker from deleting a
/// successor's entry. A worker that lost a registration race, or whose entry was
/// already replaced, finds a different owner here and leaves it alone — removing
/// it would drop the only sender for a live worker and silently strand every
/// subsequent send to that target.
pub(in crate::relay::delivery) fn unregister_worker(key: &AsyncWorkerKey, owner: WorkerOwner) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && workers.get(key).is_some_and(|entry| entry.owner == owner)
    {
        workers.remove(key);
    }
}

/// Removes all workers for `runtime_directory`, regardless of owner.
///
/// Used by `test_cleanup_acp_workers` to drop the `AsyncDeliveryRegistry` entry
/// so the sender cannot be reused. This does not reap the `acp_stub` children —
/// the `AcpTransport` worker holds the `AcpStdioClient` and is never cancelled,
/// so `AcpStdioClient::shutdown` never runs. The harness kills the `acp_stub`
/// children explicitly in `GuardedTempDir::drop` (verified: registry removal
/// alone still leaked 3 per run; with the harness kill loop the leak is 0).
pub(crate) fn remove_workers_for_runtime_directory(runtime_directory: &Path) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock() {
        workers.retain(|key, _| key.runtime_directory != runtime_directory);
    }
}

pub(in crate::relay::delivery) fn task_uses_acp_transport(task: &AsyncDeliveryTask) -> bool {
    task.bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session)
        .map(|member| matches!(member.target, TargetConfiguration::Acp(_)))
        .unwrap_or(false)
}

pub(in crate::relay::delivery) fn reserve_acp_pending_slot(pending: &AtomicUsize) -> bool {
    let mut current = pending.load(Ordering::Relaxed);
    loop {
        if current >= ACP_PENDING_MAX {
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

pub(in crate::relay::delivery) fn release_pending_slot(pending: &AtomicUsize) {
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

pub(in crate::relay::delivery) fn drop_pending_async_tasks_on_stop(
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &AtomicUsize,
    trigger: GuardTrigger,
) {
    while let Ok(task) = receiver.try_recv() {
        // Graceful shutdown keeps its own spelling, which the delivery spec
        // requires of a `Pending` member the relay still held. Every other ending
        // goes through the guard's evidence order, which reaches `not_submitted`
        // for a member that was never authorized — true, and without claiming a
        // relay shutdown that is not happening. A bundle stop reported as a
        // shutdown would tell this sender the relay is going away while it
        // carries on serving every other bundle.
        match trigger {
            GuardTrigger::GracefulShutdown => complete_task_on_shutdown(&task),
            _ => complete_task_outcome_from_trigger(&task, trigger),
        }
        release_pending_slot(pending);
    }
}
