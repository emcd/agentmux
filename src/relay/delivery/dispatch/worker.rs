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

use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendOutcome, SendResult,
};
use super::envelope::{
    build_acp_driver_services, build_coder_envelope, build_ui_envelope, build_worker_transport,
};
use super::outcomes::{collect_outcome, now_rfc3339};
use super::payload::{
    build_delivery_message, emit_envelope_metadata_inscription, resolve_target_member,
};

use crate::transports::{OutcomeFuture, SingleDeliveryOutcome, TransportHealth, TransportImpl};

use super::super::async_worker::{AsyncWorkerKey, WorkerOwner};
use super::super::fence::{FenceInProgress, FenceOutcome, FenceResolution, FenceVerdict};
use super::super::guard::{BatchId, GuardTrigger, PackingUnitId};
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
    /// When this member was authorized — the execution watchdog's anchor.
    ///
    /// Anchored at authorization rather than at the write because the bound covers
    /// the relay's whole supervised path, including the rendering and submission
    /// work between the two. Held here rather than in the ledger for the same
    /// reason the member's identity is: the worker is what can act on the bound,
    /// and it can only act on members it still holds.
    pub(super) authorized_at: Instant,
}

/// What a worker builds its transport from, fixed at spawn and reused for every
/// generation it goes on to build.
///
/// The transport *kind* is a property of the target, settled by configuration
/// before any message exists. Holding that decision here rather than reading it
/// off whichever task arrives first is what lets construction happen once at
/// spawn, and what makes a replacement generation the same construction as the
/// original rather than a second code path that has to agree with it.
pub(super) enum WorkerTransportSource {
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
pub(super) struct WorkerTransportContext {
    pub(super) namespace: String,
    pub(super) runtime_directory: std::path::PathBuf,
    pub(super) target_session: String,
    /// `None` only for a relay-wide (UI) target. A configured coder target whose
    /// member is missing fails resolution instead of arriving here as `None`,
    /// which is what lets the builder treat absence as "UI" without a second flag.
    pub(super) target_member: Option<BundleMember>,
}

impl WorkerTransportContext {
    /// Resolves a worker's construction inputs from the task electing its spawn.
    pub(super) fn resolve(task: &AsyncDeliveryTask) -> Result<Self, RelayError> {
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

async fn run_async_delivery_worker(
    key: AsyncWorkerKey,
    owner: WorkerOwner,
    mut receiver: UnboundedReceiver<AsyncDeliveryTask>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    source: WorkerTransportSource,
) {
    // Hold one `TransportImpl` for this target's lifetime. The transport KIND is
    // the only target-type-dependent decision, and it is fixed by configuration
    // (transport-abstraction spec) before this worker exists — which is why it is
    // built here, from the source the spawn site resolved, rather than off
    // whichever task happens to arrive first. Delivery is then uniform: the loop
    // submits `mailw`/`raww` for every target with no registry-based re-routing
    // and no transport-deliverability gate. ACP lifecycle, readiness mirroring,
    // and respawn live entirely in the driver and its internal task; the loop
    // never names an ACP type.
    //
    // The loop is a concurrent produce-and-collect: it submits each task to the
    // transport via the non-blocking `mailw`/`raww` seam and concurrently collects
    // the resolved `OutcomeFuture`s. Coalescing, quiescence, the token-budget
    // combine, and the blocking IO all live inside each transport's internal
    // delivery task now, so the worker no longer batches, hoists quiescence, or
    // owns `spawn_blocking`.
    let batch_settings = super::prompt_batch_settings();
    // Subscribed before any level is read, and before the transport that will
    // poke it exists. The contract requires subscribe-before-check precisely so
    // a change occurring between the check and the subscription cannot be missed;
    // creating this first makes that ordering unrepresentable rather than
    // merely observed.
    //
    // Correctness never depends on it. The authoritative state is the level the
    // worker reads, and a lost wakeup only delays a delivery to the next poll
    // below.
    let readiness_changed = std::sync::Arc::new(tokio::sync::Notify::new());
    // Observed before anything is constructed, not only inside the loop below.
    // Constructing first would start a target for a relay on its way out — a Tmux
    // pane or a Pty child spawned for a generation that can only be torn down
    // again, off-runtime and possibly blocking, delaying the exit it cannot serve.
    // The same invariant is why `initialize_acp_target_for_startup` checks
    // shutdown before creating a worker at all rather than only while waiting on
    // one. The prior lazy construction had this for free, because it ran inside
    // `submit_task` and therefore only after the loop's gate had already passed.
    if shutdown_requested() {
        shutdown_before_generation(&key, owner, &mut receiver, pending.as_ref());
        return;
    }
    // The first generation. An ACP worker starts its bootstrap here but does not
    // wait on it: awaiting made the loop below — and therefore this worker's
    // shutdown gate — unreachable for as long as the bootstrap took, which for an
    // agent parked in its `initialize` handshake is forever. The queue is held
    // until the bootstrap settles either way, so delivery semantics are unchanged;
    // what the loop gains is the ability to observe shutdown during that wait.
    let (mut transport, mut bootstrap_settled) =
        match build_generation(&key, &source, batch_settings, &readiness_changed).await {
            Ok(built) => built,
            Err(error) => {
                // Construction failed, so this worker has no transport and never
                // will. Resolve everything it was elected to carry rather than
                // installing a dead transport the health gate would then hold
                // through the whole dwell before reporting a generic unreachable.
                // Unregistering — rather than fail-stopping — is deliberate: no
                // generation ever started, so there is nothing a replacement could
                // race, and the next send is free to try again.
                //
                // Unregistering happens BEFORE the drain, and the order is the
                // whole of its correctness. `try_existing_worker` holds the
                // registry lock across `sender.send`, so once the entry is gone no
                // send can still be in flight — whereas draining first leaves the
                // entry accepting, and a send landing between the drain observing
                // empty and the unregister is lost with the receiver: no terminal
                // outcome, no quota release. That is the accept-after-drain race
                // `close_worker` exists to prevent on the shutdown path, and this
                // path has to close the same way.
                super::super::async_worker::unregister_worker(&key, owner);
                resolve_queued_tasks_with_error(&mut receiver, pending.as_ref(), &error);
                return;
            }
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
    // One member received but not yet authorized, because its target reported it
    // could not take a handover. It stays `Pending` — quota reserved, nothing
    // submitted, no batch minted — and is retried ahead of anything newer so the
    // target's FIFO order survives the wait. At most one: taking a second member
    // off the channel while this one waits would reorder the target's queue.
    let mut held: Option<AsyncDeliveryTask> = None;
    // Graceful shutdown and the execution watchdog are what fence a generation.
    // Both bounds are the relay's own `[delivery]` settings, read once per worker.
    let delivery = super::super::admission::delivery_configuration();
    let fence_observation = Duration::from_millis(delivery.fence_observation_timeout_ms);
    let submission_timeout = Duration::from_millis(delivery.submission_timeout_ms);
    // The execution watchdog's fence, present only while one is in progress. Its
    // presence is also what stops the loop submitting new work into a generation
    // it has already decided to end.
    let mut watchdog: Option<FenceInProgress> = None;
    // Set by a negative verdict and never cleared. From then on this worker
    // submits nothing, keeps its registry entry so no replacement generation can
    // be elected, and stays alive only to observe shutdown.
    let mut fail_stopped = false;
    let submit_context = SubmitContext {
        unreachable_dwell: Duration::from_millis(delivery.unreachable_dwell_ms),
    };

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
            super::super::async_worker::close_worker(&key, owner);
            // A held member was never authorized, so shutdown can say so
            // positively rather than leaving it to the guard's weaker inference.
            if let Some(task) = held.take() {
                super::super::async_worker::complete_task_on_shutdown(&task);
                super::super::async_worker::release_pending_slot(pending.as_ref());
            }
            shutdown_drain(
                &key,
                &mut transport,
                &mut inflight,
                &mut inflight_members,
                &mut receiver,
                pending.as_ref(),
                fence_observation,
            )
            .await;
            break;
        }
        // Book every collector result that is already available, before anything
        // below reads the member table.
        //
        // `inflight_members` is emptied only by `collect_outcome`, so a member
        // whose transport has already resolved it stays in that table until the
        // select below happens to pick the collect arm — and the select picks at
        // random among ready branches. Two things downstream read the table, and
        // both are wrong to read it undrained. The watchdog would arm on a member
        // that had already reported its own evidence, fencing a healthy
        // generation over a delay in the relay's own bookkeeping. Worse, the
        // verdict's cut would terminalize such a member through the trigger, so a
        // member whose unit recorded `Submitted` would be reported
        // `submission_unknown` — the evidence order inverted by nothing more than
        // scheduling.
        //
        // This is an exclusion rather than a mitigation: after this loop
        // `try_join_next_with_id` is `None` by construction, so no ready-but-
        // unbooked member can exist at either read. The select arm below stays,
        // because it is what *wakes* the loop when a collector finishes; this only
        // consumes what is ready already.
        while let Some(joined) = inflight.try_join_next_with_id() {
            collect_outcome(joined, &mut inflight_members, pending.as_ref());
        }
        if senders_dropped && inflight.is_empty() && held.is_none() && !fail_stopped {
            // No more producers, nothing in flight, and nothing waiting on
            // readiness: the worker is unreachable. A held member keeps the loop
            // alive even with its producers gone — it is still owed a delivery
            // once its target can take one, and only shutdown resolves it early.
            //
            // A fail-stopped worker never takes this exit. Leaving would
            // unregister its entry, and that entry is the whole of the
            // no-replacement guarantee; it stays until shutdown instead.
            break;
        }
        // The execution watchdog. Anchored at authorization, so a member still
        // unresolved past the bound means the relay's own supervised code has run
        // longer than it is allowed to — a statement about our execution, not
        // about the target. Nothing is terminalized here: the fence verdict is the
        // single resolution cut, and evidence arriving during the observation
        // windows still resolves its own members through the collect arm.
        if watchdog.is_none()
            && !fail_stopped
            && let Some(oldest) = inflight_members
                .values()
                .map(|member| member.authorized_at)
                .min()
            && oldest.elapsed() >= submission_timeout
        {
            emit_inscription(
                "relay.delivery.watchdog.armed",
                &json!({
                    "namespace": key.namespace,
                    "target_session": key.target_session,
                    "submission_timeout_ms": submission_timeout.as_millis() as u64,
                    "unresolved_members": inflight_members.len(),
                }),
            );
            watchdog = Some(FenceInProgress::begin(&mut transport, fence_observation));
        }
        // Advanced before the select rather than inside it, so the collect arm
        // keeps running through both observation windows: the fence's whole point
        // is that a member resolving in time reports its own evidence.
        let watchdog_verdict = watchdog
            .as_mut()
            .and_then(|fence| fence.advance(&mut transport, Instant::now()));
        if let Some(outcome) = watchdog_verdict {
            watchdog = None;
            emit_fence_verdict(&key, "submission_timeout", outcome, inflight_members.len());
            terminalize_unresolved_members(&mut inflight, &mut inflight_members);
            match outcome.verdict {
                FenceVerdict::Positive => {
                    // Cessation was observed, so nothing from the old generation
                    // can still write and a replacement is safe. Teardown follows
                    // the verdict rather than preceding it, for the same reason it
                    // does on shutdown: the cooperative step is the stop signal,
                    // and tearing down first would strip the effect paths the
                    // observation reads.
                    transport.shutdown();
                    // The replacement is the same construction as the original,
                    // from the same source, so a target cannot acquire a second
                    // transport kind by being fenced.
                    match build_generation(&key, &source, batch_settings, &readiness_changed).await
                    {
                        Ok((built, settled)) => {
                            transport = built;
                            bootstrap_settled = settled;
                        }
                        Err(error) => {
                            // No replacement could be built. The old generation
                            // did cease, so nothing can race a later worker —
                            // resolve what this one still holds and let the next
                            // send elect a fresh one, exactly as a failed first
                            // construction does.
                            //
                            // Unregistered before the drain for the same reason it
                            // is there: while the entry is live a send can land
                            // between the drain observing empty and the entry
                            // going, and it would be lost with the receiver.
                            super::super::async_worker::unregister_worker(&key, owner);
                            if let Some(task) = held.take() {
                                super::super::async_worker::complete_task_outcome(
                                    &task,
                                    Err(error.clone()),
                                );
                                super::super::async_worker::release_pending_slot(pending.as_ref());
                            }
                            resolve_queued_tasks_with_error(
                                &mut receiver,
                                pending.as_ref(),
                                &error,
                            );
                            return;
                        }
                    }
                }
                FenceVerdict::Negative => {
                    // Cessation was not observed, so an old executor may still
                    // reach the target. Refusing every further send is what holds
                    // both replacement and raw ordering — `raww` reaches this
                    // target through the same registry lookup, so it needs no
                    // barrier of its own. `shutdown` is deliberately not called:
                    // it joins the very threads that were just not observed to
                    // stop, which would run the bounded fence into the unbounded
                    // wait it exists to replace.
                    super::super::async_worker::mark_worker_fail_stopped(&key, owner);
                    fail_stopped = true;
                    if let Some(task) = held.take() {
                        super::super::async_worker::complete_task_outcome_from_trigger(
                            &task,
                            GuardTrigger::ExecutionBound,
                        );
                        super::super::async_worker::release_pending_slot(pending.as_ref());
                    }
                }
            }
        }
        // A target whose bootstrap has not settled has no runtime to submit
        // into, so its queue stays untouched; the poll arm below re-evaluates
        // this every tick.
        let bootstrap_settled_now = bootstrap_settled
            .as_ref()
            .is_none_or(|settled| settled.load(std::sync::atomic::Ordering::Acquire));
        // Retry the held member before accepting anything newer. Its target may
        // have become ready since the last attempt; the poll arm below is what
        // paces the retries, and is the bounded backstop the readiness contract
        // requires so a missed notification only delays a delivery.
        if held.is_some() && bootstrap_settled_now && watchdog.is_none() && !fail_stopped {
            let task = held.take().expect("held member present in this branch");
            held = submit_task(
                task,
                &submit_context,
                &mut transport,
                &mut inflight,
                &mut inflight_members,
                pending.as_ref(),
            );
        }
        tokio::select! {
            maybe_task = receiver.recv(), if !senders_dropped && bootstrap_settled_now && held.is_none() && watchdog.is_none() => {
                match maybe_task {
                    Some(task) => {
                        if shutdown_requested() {
                            super::super::async_worker::complete_task_on_shutdown(&task);
                            super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        if fail_stopped {
                            // Raced the registry mark: this task was handed over
                            // between the fail-stop check and the send. It was
                            // never authorized, so the evidence order proves
                            // non-delivery rather than inferring it.
                            super::super::async_worker::complete_task_outcome_from_trigger(
                                &task,
                                GuardTrigger::ExecutionBound,
                            );
                            super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        held = submit_task(
                            task,
                            &submit_context,
                            &mut transport,
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
            _ = readiness_changed.notified(), if held.is_some() => {
                // A transport reported that its observed level moved. Only
                // interesting while a member is waiting on one; otherwise the
                // loop has nothing to re-evaluate.
            }
            _ = tokio::time::sleep(poll_interval) => {
                // Poll tick: re-evaluate the shutdown gate even while idle, and
                // the lost-wakeup backstop for the notification above.
            }
        }
    }
    super::super::async_worker::unregister_worker(&key, owner);
}

/// Ends a worker that was elected before a shutdown it observed before building
/// anything.
///
/// Closes the entry first so nothing new is accepted, drains what it already
/// holds through the ordinary shutdown spelling, then unregisters — the same
/// order the loop's own shutdown drain uses, for the same reason. Deliberately
/// constructs no transport: a relay on its way out has no use for a target, and
/// starting one here would spawn a pane or a child purely to tear it down.
fn shutdown_before_generation(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    super::super::async_worker::close_worker(key, owner);
    super::super::async_worker::drop_pending_async_tasks_on_shutdown(receiver, pending);
    super::super::async_worker::unregister_worker(key, owner);
}

/// Resolves every task still queued for a worker that cannot deliver, reporting
/// the construction failure that made it so.
///
/// Used only where the worker is about to end without a transport. The tasks were
/// admitted but never authorized, so they are terminalized with the real cause
/// rather than left in a receiver nothing will poll.
fn resolve_queued_tasks_with_error(
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
    error: &RelayError,
) {
    while let Ok(task) = receiver.try_recv() {
        super::super::async_worker::complete_task_outcome(&task, Err(error.clone()));
        super::super::async_worker::release_pending_slot(pending);
    }
}

/// Records one fence verdict, naming what triggered the fence.
///
/// Shared by graceful shutdown and the execution watchdog because the fence is
/// one protocol with one vocabulary: an operator reading these should be able to
/// compare a shutdown verdict against a watchdog verdict without translating.
fn emit_fence_verdict(
    key: &AsyncWorkerKey,
    trigger: &str,
    outcome: FenceOutcome,
    unresolved_members: usize,
) {
    emit_inscription(
        "relay.delivery.fence.verdict",
        &json!({
            "namespace": key.namespace,
            "target_session": key.target_session,
            "trigger": trigger,
            "verdict": match outcome.verdict {
                FenceVerdict::Positive => "positive",
                FenceVerdict::Negative => "negative",
            },
            "resolution": match outcome.resolution {
                FenceResolution::Cooperative => "cooperative",
                FenceResolution::Forced => "forced",
                FenceResolution::Unobserved => "unobserved",
            },
            "unresolved_members": unresolved_members,
        }),
    );
}

/// Terminalizes every member still unresolved at a fence verdict, and abandons
/// the collectors that were carrying them.
///
/// The trigger says when the owning path gave up; the outcome comes from the
/// guard's evidence order, so a member whose unit was never recorded still
/// resolves `not_submitted` rather than being smeared into an unknown by the
/// bound. Collectors are aborted rather than awaited because their futures are
/// held by executors the fence has just finished establishing cannot be waited
/// on. Each abort still yields one join, and that join is what releases the
/// member's pending slot — exactly once, through the collect arm, whether or not
/// the member's identity was still in the table.
fn terminalize_unresolved_members(
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
) {
    for (_, member) in inflight_members.drain() {
        super::super::async_worker::complete_task_outcome_from_trigger(
            &member.task,
            GuardTrigger::ExecutionBound,
        );
    }
    inflight.abort_all();
}

/// Builds one generation's transport, plus the flag that says when an ACP
/// bootstrap has settled (`None` for every other kind, which has no such wait).
///
/// One function for both the worker's first generation and every replacement it
/// builds after a positive fence verdict. They must be the same construction: a
/// replacement that differed from the original would be a second transport kind
/// for one target, which the transport-abstraction contract does not allow.
async fn build_generation(
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

/// The fixed context one worker submits under.
///
/// Grouped rather than passed as parallel parameters: these are the settings the
/// worker resolves once at spawn and applies to every task, as distinct from the
/// per-task state that changes underneath them.
struct SubmitContext {
    /// How long a target may be *continuously* unreachable before a held member
    /// resolves rather than keeping its place in the queue.
    unreachable_dwell: Duration,
}

/// The outcome for a member whose target was unreachable for longer than the
/// dwell allows.
///
/// `not_submitted`, not `failed`. The member was never authorized and never
/// handed to a transport, so nothing could have reached the target — that is
/// positive evidence of non-delivery, which is exactly what `NotSubmitted`
/// asserts and what `Failed` does not.
///
/// Returned as `Ok` rather than `Err` deliberately: the error branch of the
/// terminal transition spells everything `Failed`, and only the `Ok` branch is
/// reconciled against recorded evidence. A never-authorized member has no
/// recorded evidence, so this spelling passes through as given.
fn target_unreachable_result(task: &AsyncDeliveryTask, dwell: Duration) -> SendResult {
    SendResult {
        target_session: task.target_session.clone(),
        message_id: task.message_id.clone(),
        outcome: SendOutcome::NotSubmitted,
        reason_code: Some("delivery_target_unreachable".to_string()),
        reason: Some(
            "target could not be reached for longer than the configured dwell".to_string(),
        ),
        details: Some(json!({
            "unreachable_dwell_ms": dwell.as_millis() as u64,
        })),
    }
}

/// Submits one task to its transport via the non-blocking write seam and spawns
/// an in-flight collector for its outcome. On the worker's first task the
/// transport is constructed from the target's configured `session_type()` and
/// latched (`build_worker_transport`). Delivery is then uniform: `Ui` builds the
/// stream envelope, coder transports (ACP/Tmux) render the framed envelope or
/// submit raw input, and the forward-declared `Pubsub` stub yields an explicit
/// not-implemented outcome (it is not deliverable). A construction or render
/// failure completes the task immediately and releases its slot.
///
/// Returns the task unconsumed when its target reported that it cannot take a
/// handover. Nothing has happened to it in that case — no authorization, no
/// batch, no quota movement — and the caller holds it for a later attempt.
fn submit_task(
    task: AsyncDeliveryTask,
    context: &SubmitContext,
    transport: &mut TransportImpl,
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    pending: &std::sync::atomic::AtomicUsize,
) -> Option<AsyncDeliveryTask> {
    // Readiness gates authorization, not submission. A target that cannot take a
    // handover now must not have a batch authorized against it: authorization is
    // the linearization point, and quota releases only at the terminal
    // transition, so authorizing early would commit a member to a generation that
    // cannot act on it. The member stays `Pending` instead, which is a state it
    // may occupy indefinitely — how long a target stays busy is not evidence
    // about the target, and no elapsed duration converts this wait into an
    // outcome.
    //
    // Reading the level here is deliberately advisory. It can go stale between
    // this check and the write below, and when it does the invocation fails and
    // resolves through the guard's evidence order rather than being retried
    // behind the sender's back.
    //
    // Pubsub is excluded rather than gated. It reports unready like any transport
    // with no delivery path, so gating it would hold a member no transport can
    // ever accept; it is rejected synchronously below instead.
    if !matches!(transport, TransportImpl::Pubsub) {
        // Both axes are required, and neither substitutes for the other. Health
        // is read first because it is what bounds the wait, and an unreachable
        // target is never authorized whatever readiness says — a transport that
        // cannot reach its target has nothing useful to report about whether that
        // target is at a prompt.
        match transport.health() {
            // Continuously unreachable past the dwell. Waiting will not make this
            // target ready, so the member resolves rather than keeping a place in
            // a queue nothing will drain.
            TransportHealth::Unreachable { since }
                if since.elapsed() >= context.unreachable_dwell =>
            {
                super::super::async_worker::complete_task_outcome(
                    &task,
                    Ok(target_unreachable_result(&task, context.unreachable_dwell)),
                );
                super::super::async_worker::release_pending_slot(pending);
                return None;
            }
            // Unreachable but not yet past the dwell: hold, exactly as an unready
            // target is held. An unreachability that ends in time costs nothing.
            TransportHealth::Unreachable { .. } => return Some(task),
            TransportHealth::Healthy => {}
        }
        if !transport.is_ready_for_handover() {
            return Some(task);
        }
    }

    // Authorization is the linearization point and the watchdog's anchor, so it
    // happens once, before any transport-specific branch, and the clock starts
    // with it. Starting the clock after the write would exclude the synchronous
    // rendering and submission work the bound is supposed to cover.
    let authorized_at = Instant::now();
    if !matches!(transport, TransportImpl::Pubsub) {
        authorize_member(&task);
    }

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
        return None;
    } else if matches!(transport, TransportImpl::Ui(_)) {
        let envelope = build_ui_envelope(&task);
        // Recorded immediately before the call that can produce an effect, and
        // never earlier: the gap between authorization and this point is exactly
        // the window in which the guard can still prove nothing was written.
        super::super::admission::bind_to_packing_unit(
            task.message_id.as_str(),
            PackingUnitId::mint(),
        );
        (transport.mailw(envelope), false)
    } else {
        // Every coder submission marks handover too. Omitting it here let a
        // coder member that panicked *after* its write resolve `not_submitted` —
        // a positive claim that nothing reached the target — when bytes may well
        // have landed. That is the exact inversion the evidence order exists to
        // prevent.
        match prepare_coder_write(&task, transport) {
            Ok(future) => (future, true),
            Err(error) => {
                // Refused before any target-side effect, so nothing reached the
                // target and reporting the refusal is sound.
                //
                // Note what this does *not* do: an explicit `Err` bypasses the
                // guard's evidence order, so this reports `failed` rather than
                // the `not_submitted` that order would derive from the member
                // being unbound. Both are true — nothing was written — but
                // `failed` is the undifferentiated spelling and `not_submitted`
                // is the positive one, so the sender is told less than the relay
                // can prove. Left as-is deliberately: changing an outcome
                // spelling is contract-visible and belongs in a reviewed change,
                // not a comment fix.
                super::super::async_worker::complete_task_outcome(&task, Err(error));
                super::super::async_worker::release_pending_slot(pending);
                return None;
            }
        }
    };

    let handle = inflight.spawn(async move { future.await.ok() });
    inflight_members.insert(
        handle.id(),
        InflightMember {
            task,
            record_served,
            authorized_at,
        },
    );
    None
}

/// Performs the `Pending` to `Authorized` transition for one member, minting its
/// batch and guard identities.
///
/// One member per batch today: the worker submits tasks one at a time, so a
/// batch is a batch of one until batch formation against the transport's
/// maximum handover dimensions lands and starts forming real ones.
fn authorize_member(task: &AsyncDeliveryTask) {
    super::super::admission::authorize(task.message_id.as_str(), BatchId::mint());
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
            let envelope = build_coder_envelope(task, message);
            // Bound immediately before the call that can produce a target-side
            // effect. Everything above this line is relay-local rendering, so a
            // failure there is still provably non-delivery.
            super::super::admission::bind_to_packing_unit(
                task.message_id.as_str(),
                PackingUnitId::mint(),
            );
            Ok(transport.mailw(envelope))
        }
        DeliveryPayloadMode::RawInput => {
            super::super::admission::bind_to_packing_unit(
                task.message_id.as_str(),
                PackingUnitId::mint(),
            );
            Ok(transport.raww(task.message.clone(), task.append_enter))
        }
    }
}

/// Drains the worker on relay shutdown, bounded by the same fence the watchdog
/// uses.
///
/// Graceful shutdown ends a generation, so it establishes cessation the way
/// every other generation ending does — and it carries a bound for the same
/// reason. Waiting on collectors until they happened to finish made the relay's
/// exit hostage to an executor blocked in a syscall, which is exactly the class
/// of wait the fence exists to replace: no runtime primitive can force such a
/// thread to return, so *observing* it is the only sound move and observing has
/// to be budgeted.
///
/// Collection runs alongside both observation windows rather than after the
/// verdict, so a member whose transport resolves in time still reports its own
/// evidence instead of a shutdown-shaped guess. The verdict is the cut; whatever
/// is unresolved at it terminalizes through the guard's evidence order.
///
/// The transport's own teardown follows the verdict rather than preceding it,
/// and only on a positive one. The fence's cooperative step is already every
/// transport's stop signal — the dropped shutdown channel, the shutdown flag,
/// the fenced generation — so tearing resources down first would be the
/// destructive action before the polite one, and would strip the effect paths
/// the observation reads.
///
/// A negative verdict skips teardown entirely. `shutdown` reaps children and
/// joins threads, and a negative verdict is precisely the finding that those
/// threads were not observed to stop — so calling it there would run the bounded
/// fence straight into the unbounded wait it exists to replace. Cessation has
/// already been initiated by the fence's forced step; what is abandoned is the
/// waiting, and the process is exiting anyway.
async fn shutdown_drain(
    key: &AsyncWorkerKey,
    transport: &mut TransportImpl,
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
    fence_observation: Duration,
) {
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    let mut fence = FenceInProgress::begin(transport, fence_observation);
    let outcome = loop {
        if let Some(outcome) = fence.advance(transport, Instant::now()) {
            break outcome;
        }
        tokio::select! {
            joined = inflight.join_next_with_id(), if !inflight.is_empty() => {
                if let Some(joined) = joined {
                    collect_outcome(joined, inflight_members, pending);
                }
            }
            _ = tokio::time::sleep(poll_interval) => {}
        }
    };
    emit_fence_verdict(key, "graceful_shutdown", outcome, inflight_members.len());
    if outcome.verdict == FenceVerdict::Positive {
        transport.shutdown();
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
