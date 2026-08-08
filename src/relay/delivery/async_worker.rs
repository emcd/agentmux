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
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, error::SendError};

use crate::configuration::{BundleConfiguration, BundleMember, TargetConfiguration};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::runtime::{inscriptions::emit_inscription, signals::shutdown_requested};
use crate::tmux::TmuxOutputView;
use crate::transports::{OutputView, WorkerFailureReason, WorkerReadinessState};

use super::admission::TerminalTransition;
use super::guard::{GuardKey, GuardTrigger, SubmissionEvidence};

use super::super::stream::{RelayStreamEvent, send_event_to_registered_ui};
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RELAY_NAMESPACE, RelayError, SCHEMA_VERSION,
    SendOutcome, SendResult, SenderReturnRoute, canonical_session_id,
};

use std::path::{Path, PathBuf};

const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const ACP_ERROR_CODE_QUEUE_FULL: &str = "runtime_acp_queue_full";
const ACP_PENDING_MAX: usize = 64;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct AsyncWorkerKey {
    pub runtime_directory: PathBuf,
    pub namespace: String,
    pub target_session: String,
}

#[derive(Default)]
pub(super) struct AsyncDeliveryRegistry {
    pub workers: Mutex<HashMap<AsyncWorkerKey, AsyncWorkerEntry>>,
}

pub(super) struct AsyncWorkerEntry {
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
}

pub(super) fn build_worker_key(
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

/// Outcome of handing a task to an already-registered worker without spawning
/// one. Distinguishes the task landing in a live worker from the two no-op cases
/// the caller must treat differently: a *missing* worker (spawn one, or report
/// the ACP target unavailable) versus a *closing* worker draining for shutdown
/// (drop the task best-effort; never resurrect a worker mid-shutdown). Collapsing
/// the two into one "bounced" case lets a Send racing shutdown clobber a closing
/// entry the shutdown barrier still counts.
pub(super) enum WorkerDispatch {
    /// The task was accepted by a live, non-closing worker.
    Accepted,
    /// No live worker is registered for the key (absent, or its receiver was
    /// dropped); the task is returned so the caller can spawn one or, for an ACP
    /// target, report it unavailable.
    Missing(AsyncDeliveryTask),
    /// The worker is draining for shutdown and will not poll its receiver again;
    /// the task is returned so the caller drops it best-effort.
    Closing(AsyncDeliveryTask),
}

pub(super) fn try_existing_worker(
    key: &AsyncWorkerKey,
    task: AsyncDeliveryTask,
) -> Result<WorkerDispatch, RelayError> {
    let registry = async_delivery_registry();
    let mut workers = registry.workers.lock().map_err(|_| {
        super::super::relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;

    if let Some(worker) = workers.get(key) {
        if worker.closing {
            // The worker is draining for shutdown and will not poll its receiver
            // again; bounce the task rather than accepting it into a dead queue.
            return Ok(WorkerDispatch::Closing(task));
        }
        if worker.bounded_acp_queue && !reserve_acp_pending_slot(worker.pending.as_ref()) {
            return Err(super::super::relay_error(
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
                return Ok(WorkerDispatch::Missing(returned));
            }
        }
    }
    Ok(WorkerDispatch::Missing(task))
}

/// Identifies one worker installation, distinct from every other for the same
/// target. Minted by whichever caller wins the registry insert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WorkerOwner(u64);

static NEXT_WORKER_OWNER: AtomicU64 = AtomicU64::new(1);

impl WorkerOwner {
    fn mint() -> Self {
        Self(NEXT_WORKER_OWNER.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
pub(super) fn register_worker(
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
            },
        );
    }
    owner
}

pub(super) fn register_worker_if_absent(
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: std::sync::Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> Result<Option<WorkerOwner>, RelayError> {
    let mut workers = async_delivery_registry().workers.lock().map_err(|_| {
        super::super::relay_error(
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
        },
    );
    Ok(Some(owner))
}

/// Marks a worker as closing so it bounces new sends while its entry stays
/// registered, preserving the shutdown-barrier worker count until the worker
/// finishes draining and unregisters. Called at the start of the shutdown drain
/// to close the accept-after-drain race without dropping the count early.
pub(super) fn close_worker(key: &AsyncWorkerKey, owner: WorkerOwner) {
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
    super::observability::publish_worker_readiness(&key, state);
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

pub(in crate::relay) fn acp_session_ready_for_startup(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> bool {
    matches!(
        get_worker_readiness(namespace, runtime_directory, target_session),
        Some(WorkerReadinessState::Available)
    )
}

/// Removes this target's registry entry, but only if `owner` still holds it.
///
/// The ownership check is what keeps an exiting worker from deleting a
/// successor's entry. A worker that lost a registration race, or whose entry was
/// already replaced, finds a different owner here and leaves it alone — removing
/// it would drop the only sender for a live worker and silently strand every
/// subsequent send to that target.
pub(super) fn unregister_worker(key: &AsyncWorkerKey, owner: WorkerOwner) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && workers.get(key).is_some_and(|entry| entry.owner == owner)
    {
        workers.remove(key);
    }
}

pub(super) fn task_uses_acp_transport(task: &AsyncDeliveryTask) -> bool {
    task.bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session)
        .map(|member| matches!(member.target, TargetConfiguration::Acp(_)))
        .unwrap_or(false)
}

pub(super) fn reserve_acp_pending_slot(pending: &AtomicUsize) -> bool {
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

/// Attempts the terminal transition and decides whether this caller may report.
///
/// `None` means stay silent: either another caller already won the transition,
/// or no reservation exists for a task that *should* have had one — which means
/// the winner already terminalized it and cleaned the ledger entry up.
///
/// The winning transition removes the ledger entry, so absence cannot by itself
/// distinguish "already resolved by someone else" from "never admitted". Only
/// the task can: a relay-originated receipt is the one thing that legitimately
/// bypasses admission. Reporting on absence alone is what let two competing
/// resolvers each emit an outcome for a single accepted member.
///
/// `Some((None, _))` is the receipt case — reportable, with no recorded evidence
/// because nothing was ever admitted to record any against.
fn resolve_terminal_transition(
    task: &AsyncDeliveryTask,
) -> Option<(Option<SubmissionEvidence>, Option<GuardKey>)> {
    match super::admission::terminalize(task.message_id.as_str()) {
        TerminalTransition::Won { evidence, guard } => Some((Some(evidence), guard)),
        TerminalTransition::AlreadyTerminal => None,
        // Never admitted, so nothing else can be racing to resolve it.
        TerminalTransition::NoReservation if !task.admitted => Some((None, None)),
        // Admitted, but its reservation is gone: the winner already terminalized
        // it and cleaned up. Reporting here would be the duplicate.
        TerminalTransition::NoReservation => None,
    }
}

/// Terminalizes one member because a lifecycle trigger fired, with no outcome of
/// its own to report.
///
/// The trigger does not choose the outcome — the guard's evidence order does.
/// That separation is the point: a collector panic on a member that was never
/// handed to a transport resolves `not_submitted`, because the relay can prove
/// it, while the same panic after handover resolves `submission_unknown`.
pub(super) fn complete_task_outcome_from_trigger(task: &AsyncDeliveryTask, trigger: GuardTrigger) {
    let Some((evidence, guard)) = resolve_terminal_transition(task) else {
        return;
    };
    // A receipt carries no recorded evidence, so a trigger on one can only be
    // reported honestly as unknown.
    let evidence = evidence.unwrap_or(SubmissionEvidence::SubmissionUnknown);
    report_terminal_outcome(
        task,
        Ok(SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: evidence.outcome(),
            reason_code: Some(evidence.reason_code().to_string()),
            reason: Some(trigger.reason().to_string()),
            details: None,
        }),
        guard,
    );
}

pub(super) fn complete_task_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<SendResult, RelayError>,
) {
    // The single terminal transition, and the one place admission quota is
    // released. Every path that can finish a member routes through here, so
    // being told to stay silent means another path already resolved this member:
    // emitting anything below would be the duplicate resolution the guard exists
    // to prevent.
    let Some((evidence, guard)) = resolve_terminal_transition(task) else {
        return;
    };
    // The transport's outcome is the evidence, so it is reported as given —
    // except where the recorded evidence is strictly more honest than the
    // spelling the transport chose. An undifferentiated failure cannot support a
    // claim of non-delivery, so it surfaces as `submission_unknown` rather than
    // as `failed` or `timeout`.
    let outcome = outcome.map(|result| match evidence {
        Some(evidence) => reconcile_with_evidence(result, evidence),
        None => result,
    });
    report_terminal_outcome(task, outcome, guard);
}

/// Replaces a reported outcome's spelling with the guard's recorded evidence
/// where the evidence says something the spelling cannot support.
///
/// `Delivered` and `NotSubmitted` are positive claims a transport is entitled to
/// make and are left alone. Everything else is an undifferentiated failure from
/// the guard's point of view: partial effect cannot be excluded, so reporting it
/// as a failure would assert a non-delivery the relay cannot stand behind.
/// `dropped_on_shutdown` is preserved as its own spelling, because it names when
/// the relay stopped rather than what happened at the target.
fn reconcile_with_evidence(result: SendResult, evidence: SubmissionEvidence) -> SendResult {
    if evidence != SubmissionEvidence::SubmissionUnknown
        || matches!(
            result.outcome,
            SendOutcome::Delivered | SendOutcome::NotSubmitted | SendOutcome::DroppedOnShutdown
        )
    {
        return result;
    }
    SendResult {
        outcome: SendOutcome::SubmissionUnknown,
        reason_code: result
            .reason_code
            .or_else(|| Some(evidence.reason_code().to_string())),
        ..result
    }
}

/// Emits the observability floor for a resolved member and routes its receipt.
///
/// Reached only after the terminal transition has been won, so everything it
/// emits happens exactly once per member.
fn report_terminal_outcome(
    task: &AsyncDeliveryTask,
    outcome: Result<SendResult, RelayError>,
    guard: Option<GuardKey>,
) {
    // The terminal outcome (and its reason), independent of the Ok/Err shape, so a
    // non-delivered outcome can be routed back to the sender as a receipt after the
    // observability floor is emitted below. A relay-side delivery error resolves to
    // `Failed`, matching the inscription arm.
    let (terminal_outcome, terminal_reason_code, terminal_reason) = match &outcome {
        Ok(result) => (
            result.outcome.clone(),
            result.reason_code.clone(),
            result.reason.clone(),
        ),
        Err(error) => (
            SendOutcome::Failed,
            Some(error.code.clone()),
            Some(error.message.clone()),
        ),
    };
    match outcome {
        Ok(result) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_namespace.as_str(),
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
                    "namespace": task.bundle.bundle_name,
                    "sender_session": task.sender.id,
                    "target_session": result.target_session,
                    "message_id": result.message_id,
                    "outcome": result.outcome,
                    "reason_code": result.reason_code,
                    "reason": result.reason,
                    "details": result.details,
                    "batch_id": guard.map(|key| key.batch.value()),
                    "attempt_id": guard.map(|key| key.attempt.value()),
                }),
            );
        }
        Err(error) => {
            emit_sender_delivery_outcome_event(
                task.bundle.bundle_name.as_str(),
                task.sender_namespace.as_str(),
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
                    "namespace": task.bundle.bundle_name,
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
    // With the observability floor recorded above for every terminal outcome,
    // best-effort a terminal-outcome receipt back to the sender for the
    // non-delivered ones.
    deliver_terminal_outcome_receipt(
        task,
        &terminal_outcome,
        terminal_reason_code.as_deref(),
        terminal_reason.as_deref(),
    );
}

/// The relay/system principal that a terminal-outcome receipt is attributed to,
/// rendered as `relay@RELAY` in the receipt envelope so a recipient can tell it
/// from inbound peer traffic.
const TERMINAL_RECEIPT_SENDER_ID: &str = "relay";

/// Whether a terminal outcome is *non-delivered* and therefore warrants a
/// receipt. `Delivered` is success (recorded on the floor only), `Queued` is not
/// terminal, and `PeerUnavailable` is a cross-relay outcome reported
/// synchronously on the send response — none produce a receipt.
fn is_non_delivered_outcome(outcome: &SendOutcome) -> bool {
    matches!(
        outcome,
        SendOutcome::Failed
            | SendOutcome::Timeout
            | SendOutcome::DroppedOnShutdown
            // Both are terminal non-delivery outcomes and owe the sender a
            // receipt exactly as much as the others do. `not_submitted` is a
            // sound assertion that nothing arrived; `submission_unknown` is the
            // absence of one, which a sender arguably needs to hear about more
            // urgently than a plain failure, not less.
            | SendOutcome::NotSubmitted
            | SendOutcome::SubmissionUnknown
    )
}

/// Best-effort delivers a terminal-outcome receipt back to the original sender
/// when a queued message resolves to a non-delivered terminal outcome.
///
/// This is the single non-recursion chokepoint: a receipt is spawned only when
/// the resolving delivery is itself not a receipt (`is_receipt == false`) and its
/// outcome is non-delivered. The receipt routes ONLY to the sender's already-live
/// delivery worker via [`try_existing_worker`], which bounces the task back
/// rather than spawning a worker when the sender is not routable — so an
/// unreachable sender's receipt is dropped, never persisted or retried. The
/// terminal outcome is already on the `relay.log` floor regardless.
fn deliver_terminal_outcome_receipt(
    task: &AsyncDeliveryTask,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) {
    if task.is_receipt || !is_non_delivered_outcome(outcome) {
        return;
    }
    // A sender with no home bundle (`GLOBAL`/`RELAY`) carries no return route; a
    // UI operator among them is served by the `delivery_outcome` stream frame the
    // observability path already emitted.
    let Some(route) = task.sender_return_route.as_ref() else {
        return;
    };
    let receipt = build_terminal_outcome_receipt(task, route, outcome, reason_code, reason);
    let sender_key = build_worker_key(
        task.sender_namespace.as_str(),
        route.runtime_directory.as_path(),
        route.member.id.as_str(),
    );
    // Route to the sender's live worker only; drop on any non-delivery (no live
    // worker, or a bounded ACP queue that is full). Never spawn a worker for a
    // receipt — that is the deliberate boundary against deferred delivery.
    let _ = try_existing_worker(&sender_key, receipt);
}

/// Builds the receipt delivery task addressed back to the original sender. The
/// receipt carries a minimal delivery bundle holding just the sender's real
/// member, so the delivery worker resolves the sender's true transport and
/// renders/delivers the receipt through it. It is attributed to the relay/system
/// principal and marked `is_receipt` so its own terminal outcome spawns nothing.
fn build_terminal_outcome_receipt(
    task: &AsyncDeliveryTask,
    route: &SenderReturnRoute,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> AsyncDeliveryTask {
    let sender_namespace = task.sender_namespace.clone();
    let sender_id = route.member.id.clone();
    let body = render_receipt_body(task, outcome, reason_code, reason);
    // A minimal delivery bundle carrying only the original sender as the receipt's
    // target: enough for the worker to resolve the sender's real member (its true
    // transport) and deliver through it, with no co-recipients.
    let bundle = BundleConfiguration {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: sender_namespace.clone(),
        autostart: false,
        groups: Vec::new(),
        members: vec![route.member.clone()],
    };
    AsyncDeliveryTask {
        bundle,
        // A receipt bypasses admission entirely: nothing was accepted for it, so
        // it holds no reservation and stays reportable on its own terms.
        admitted: false,
        // Relay/system origin (`relay@RELAY`), not a peer principal.
        sender_namespace: RELAY_NAMESPACE.to_string(),
        sender: relay_system_sender_member(),
        authenticated_identity: None,
        on_behalf_of: None,
        all_target_sessions: vec![canonical_session_id(
            sender_id.as_str(),
            sender_namespace.as_str(),
        )],
        target_session: sender_id,
        message: body,
        message_id: uuid::Uuid::new_v4().to_string(),
        quiescence: super::QuiescenceOptions::for_async(None),
        runtime_directory: route.runtime_directory.clone(),
        payload_mode: DeliveryPayloadMode::EnvelopeMessage,
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: true,
        sender_return_route: None,
    }
}

/// The synthetic relay/system sender member for a receipt envelope. Its
/// `target` is inert: the receipt's transport is built from the receipt's target
/// member (the original sender), never from this sender member.
fn relay_system_sender_member() -> BundleMember {
    BundleMember {
        id: TERMINAL_RECEIPT_SENDER_ID.to_string(),
        name: Some(TERMINAL_RECEIPT_SENDER_ID.to_string()),
        working_directory: None,
        target: TargetConfiguration::Ui,
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    }
}

/// Renders the human-readable receipt body naming the original `message_id`, the
/// delivery target, the non-delivered outcome, and any `reason_code`/`reason`, so
/// the sender can correlate it to the `queued` result it received at accept time.
fn render_receipt_body(
    task: &AsyncDeliveryTask,
    outcome: &SendOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> String {
    let target = canonical_session_id(
        task.target_session.as_str(),
        task.bundle.bundle_name.as_str(),
    );
    let outcome_word = match outcome {
        SendOutcome::Timeout => "timed out",
        SendOutcome::DroppedOnShutdown => "dropped on relay shutdown",
        _ => "failed",
    };
    let mut body = format!(
        "Your message {message_id} to {target} was not delivered ({outcome_word}).",
        message_id = task.message_id,
    );
    if let Some(reason_code) = reason_code {
        body.push_str(&format!(" reason_code={reason_code}"));
    }
    if let Some(reason) = reason {
        body.push_str(&format!(": {reason}"));
    }
    body
}

/// Routes a `delivery_outcome` event for one task back to the sender within its
/// home bundle. Takes the sender/target identity as discrete strings rather than
/// the whole task so ACP completion closures — which hold only cloned per-task
/// fields, not a `&AsyncDeliveryTask` that lives long enough — can emit terminal
/// outcomes once the agent turn finishes.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_sender_delivery_outcome_event(
    target_namespace: &str,
    sender_namespace: &str,
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
        // A cross-relay peer-unavailable outcome is reported synchronously on the
        // send response, never through this local async terminal-outcome path; the
        // arm is defensive so the outcome maps honestly if it ever reaches here.
        SendOutcome::PeerUnavailable => ("failed", Some("peer_unavailable")),
        // Both terminal, and both distinct from `failed`. `not_submitted` is a
        // sound assertion of non-delivery; `submission_unknown` is the absence of
        // one, so neither may be collapsed into a failure spelling that would
        // claim more than the evidence supports.
        SendOutcome::NotSubmitted => ("not_submitted", Some("not_submitted")),
        SendOutcome::SubmissionUnknown => ("submission_unknown", Some("submission_unknown")),
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
        target_session: canonical_session_id(target_session, target_namespace),
        created_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        payload: serde_json::Value::Object(payload),
    };
    // Route the sender's delivery-outcome event back to the sender within its
    // home bundle, which differs from the target's bundle for cross-bundle sends.
    let _ = send_event_to_registered_ui(sender_namespace, sender_session, &event);
}

#[cfg(test)]
mod tests {
    // These tests drive the terminal-resolution chokepoint
    // (`complete_task_outcome` -> `deliver_terminal_outcome_receipt`) directly and
    // observe the routed receipt — or its deliberate absence — on registered worker
    // channels. The chokepoint is crate-private and no public seam can isolate its
    // sender-route and non-recursion behavior: the only end-to-end exerciser is the
    // real-tmux send pipeline, and asserting the *absence* of a receipt there would
    // require reading the very sender pane the failing (non-delivered) case wedges.
    // `terminal_outcome_receipt_routes_to_sender_worker_not_target` proves the
    // receipt is keyed to `sender_return_route`;
    // `a_receipt_outcome_spawns_no_second_receipt` proves the non-recursion gate.
    use super::*;

    fn tmux_member(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Tmux(crate::configuration::TmuxTargetConfiguration {
                start_command: format!("run-{id}"),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    /// A non-delivered terminal outcome spawns a receipt keyed to the SENDER's
    /// worker — its namespace, runtime directory, and member — and delivers it
    /// there, while a worker registered at the (deliberately different) target key
    /// receives nothing. A receipt built from the target's route would miss the
    /// sender's worker entirely, which is exactly how a cross-bundle receipt would
    /// misroute.
    #[test]
    fn terminal_outcome_receipt_routes_to_sender_worker_not_target() {
        let sender_namespace = "receipt-sender-ns";
        let sender_runtime = "/receipt-sender-rt";
        let sender_member_id = "receipt-sender-mem";
        let target_namespace = "receipt-target-ns";
        let target_runtime = "/receipt-target-rt";
        let target_session = "receipt-target-sess";

        // A live sender worker (the receipt's intended destination) plus a decoy
        // worker at the target key that must NOT receive the receipt. Held receivers
        // keep both channels open.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let sender_owner = register_worker(
            sender_key.clone(),
            sender_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        let target_key =
            build_worker_key(target_namespace, Path::new(target_runtime), target_session);
        let (target_tx, mut target_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            target_key.clone(),
            target_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );

        // The task's delivery context (bundle + runtime) is the target's; its
        // return route is the sender's, with a runtime distinct from the target's.
        let task = AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: target_namespace.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: sender_namespace.to_string(),
            sender: relay_system_sender_member(),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: target_session.to_string(),
            message: "body".to_string(),
            message_id: "orig-message-id".to_string(),
            quiescence: crate::relay::delivery::QuiescenceOptions::for_async(None),
            runtime_directory: PathBuf::from(target_runtime),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: Some(SenderReturnRoute {
                member: tmux_member(sender_member_id),
                runtime_directory: PathBuf::from(sender_runtime),
            }),
        };

        complete_task_outcome(
            &task,
            Ok(SendResult {
                target_session: target_session.to_string(),
                message_id: "orig-message-id".to_string(),
                outcome: SendOutcome::Timeout,
                reason_code: Some("delivery_prime_timeout".to_string()),
                reason: Some("no output before prime timeout".to_string()),
                details: None,
            }),
        );

        let receipt = sender_rx
            .try_recv()
            .expect("a non-delivered outcome routes a receipt to the sender's worker");
        assert!(receipt.is_receipt, "the routed task is a receipt");
        assert_eq!(receipt.target_session, sender_member_id);
        assert_eq!(receipt.sender_namespace, RELAY_NAMESPACE);
        assert!(
            receipt.message.contains("orig-message-id"),
            "receipt names the original message id: {}",
            receipt.message
        );
        assert!(
            receipt
                .message
                .contains(&canonical_session_id(target_session, target_namespace)),
            "receipt names the delivery target: {}",
            receipt.message
        );
        // The worker at the TARGET key received nothing: the receipt is keyed to the
        // sender, never the target.
        assert!(
            target_rx.try_recv().is_err(),
            "the receipt must not route to a worker at the target key"
        );

        unregister_worker(&sender_key, sender_owner);
        unregister_worker(&target_key, owner);
    }

    /// A receipt's own terminal outcome spawns no further receipt (non-recursion).
    /// The sender worker is live and routable and the outcome is non-delivered — so
    /// the ONLY thing that keeps a second receipt off its channel is the
    /// `is_receipt` gate at the single spawn site. Without that gate a receipt for a
    /// receipt would land here.
    #[test]
    fn a_receipt_outcome_spawns_no_second_receipt() {
        let sender_namespace = "recursion-sender-ns";
        let sender_runtime = "/recursion-sender-rt";
        let sender_member_id = "recursion-sender-mem";

        // A live, routable sender worker: if a receipt were (wrongly) spawned for
        // this already-a-receipt task, it would route here and the assertion below
        // would catch it.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            sender_key.clone(),
            sender_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );

        // The resolving delivery is itself a receipt (`is_receipt: true`) that
        // resolves to a non-delivered outcome — the case that, but for the gate,
        // would recurse.
        let task = AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: sender_namespace.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: sender_namespace.to_string(),
            sender: relay_system_sender_member(),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: sender_member_id.to_string(),
            message: "receipt body".to_string(),
            message_id: "receipt-message-id".to_string(),
            quiescence: crate::relay::delivery::QuiescenceOptions::for_async(None),
            runtime_directory: PathBuf::from(sender_runtime),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: true,
            sender_return_route: Some(SenderReturnRoute {
                member: tmux_member(sender_member_id),
                runtime_directory: PathBuf::from(sender_runtime),
            }),
        };

        complete_task_outcome(
            &task,
            Ok(SendResult {
                target_session: sender_member_id.to_string(),
                message_id: "receipt-message-id".to_string(),
                outcome: SendOutcome::Timeout,
                reason_code: Some("target_never_settled".to_string()),
                reason: Some("tmux target did not become ready".to_string()),
                details: None,
            }),
        );

        assert!(
            matches!(
                sender_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "a receipt's own non-delivered outcome must not spawn a second receipt; \
             the sender channel must stay connected and empty (a Disconnected here \
             would mean the worker was never live, voiding the proof)"
        );

        unregister_worker(&sender_key, owner);
    }
}
