//! Ending a worker: the pre-generation exit, the fence-bounded drain, and the
//! records both leave behind.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde_json::json;

use tokio::{sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::relay::{AsyncDeliveryTask, RelayError};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::budget_within_shutdown;
use crate::transports::TransportImpl;

use super::super::super::async_worker::{AsyncWorkerKey, WorkerOwner};
use super::super::super::fence::{FenceInProgress, FenceOutcome, FenceResolution, FenceVerdict};
use super::super::super::guard::GuardTrigger;
use super::super::outcomes::collect_outcome;
use super::spawn::{
    ASYNC_WORKER_POLL_INTERVAL_MS, InflightMember, InflightOutcome, SHUTDOWN_FENCE_RESERVE_MS,
};

/// Ends a worker that was elected before a shutdown it observed before building
/// anything.
///
/// Closes the entry first so nothing new is accepted, drains what it already
/// holds through the ordinary shutdown spelling, then unregisters — the same
/// order the loop's own shutdown drain uses, for the same reason. Deliberately
/// constructs no transport: a relay on its way out has no use for a target, and
/// starting one here would spawn a pane or a child purely to tear it down.
pub(super) fn shutdown_before_generation(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
) {
    super::super::super::async_worker::close_worker(key, owner);
    // Reached only under `shutdown_requested()`, so the shutdown spelling is the
    // true one here rather than a default.
    super::super::super::async_worker::drop_pending_async_tasks_on_stop(
        receiver,
        pending,
        GuardTrigger::GracefulShutdown,
    );
    super::super::super::async_worker::unregister_worker(key, owner);
}

/// Resolves every task still queued for a worker that cannot deliver, reporting
/// the construction failure that made it so.
///
/// Used only where the worker is about to end without a transport. The tasks were
/// admitted but never authorized, so they are terminalized with the real cause
/// rather than left in a receiver nothing will poll.
pub(super) fn resolve_queued_tasks_with_error(
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
    error: &RelayError,
) {
    while let Ok(task) = receiver.try_recv() {
        super::super::super::async_worker::complete_task_outcome(&task, Err(error.clone()));
        super::super::super::async_worker::release_pending_slot(pending);
    }
}

/// Records one fence verdict, naming what triggered the fence.
///
/// Shared by graceful shutdown and the execution watchdog because the fence is
/// one protocol with one vocabulary: an operator reading these should be able to
/// compare a shutdown verdict against a watchdog verdict without translating.
pub(super) fn emit_fence_verdict(
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
pub(super) fn terminalize_unresolved_members(
    inflight: &mut JoinSet<InflightOutcome>,
    inflight_members: &mut HashMap<tokio::task::Id, InflightMember>,
) {
    for (_, member) in inflight_members.drain() {
        super::super::super::async_worker::complete_task_outcome_from_trigger(
            &member.task,
            GuardTrigger::ExecutionBound,
        );
    }
    inflight.abort_all();
}

/// What a worker is still holding when it is told to end.
///
/// Grouped because they are one thing — the work this worker owns — and are only
/// ever passed together. The drain has to reach all four: the writes still in
/// flight, the members they belong to, the queue nothing will poll again, and the
/// counter those members reserved against.
pub(super) struct WorkerHoldings<'a> {
    pub(super) inflight: &'a mut JoinSet<InflightOutcome>,
    pub(super) inflight_members: &'a mut HashMap<tokio::task::Id, InflightMember>,
    pub(super) receiver: &'a mut UnboundedReceiver<AsyncDeliveryTask>,
    pub(super) pending: &'a std::sync::atomic::AtomicUsize,
}

/// Why a worker is ending: the relay is exiting, or this worker's bundle is being
/// torn down while the relay keeps serving others.
///
/// Both end a generation and therefore establish cessation the same way; what
/// differs is only what the relay tells the operator and the member's sender. A
/// bundle stop is not a relay shutdown, and reporting it as one would name the
/// wrong event on a relay that is still serving every other bundle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StopCause {
    Shutdown,
    BundleStop,
}

impl StopCause {
    /// The label recorded on this ending's generation-fence verdict.
    fn fence_trigger(self) -> &'static str {
        match self {
            Self::Shutdown => "graceful_shutdown",
            Self::BundleStop => "bundle_stop",
        }
    }

    /// The trigger through which a member still unresolved at the verdict is
    /// terminalized.
    pub(super) fn guard_trigger(self) -> GuardTrigger {
        match self {
            Self::Shutdown => GuardTrigger::GracefulShutdown,
            Self::BundleStop => GuardTrigger::BundleStop,
        }
    }
}

/// Drains the worker when it is told to end, bounded by the same fence the
/// watchdog uses.
///
/// An ending — relay shutdown or a stop of this worker's bundle — ends a
/// generation, so it establishes cessation the way every other generation ending
/// does, and it carries a bound for the same reason. Waiting on collectors until
/// they happened to finish made the relay's exit hostage to an executor blocked
/// in a syscall, which is exactly the class of wait the fence exists to replace:
/// no runtime primitive can force such a thread to return, so *observing* it is
/// the only sound move and observing has to be budgeted.
///
/// The budget is computed identically for both causes. On a bundle stop no
/// process deadline exists, so `budget_within_shutdown` yields the configured
/// observation; the halving still applies, because the fence spends two windows
/// either way and a uniform bound is worth more here than the wider window a
/// bundle stop could afford.
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
pub(super) async fn stop_drain(
    key: &AsyncWorkerKey,
    transport: &mut TransportImpl,
    holdings: WorkerHoldings<'_>,
    fence_observation: Duration,
    cause: StopCause,
) {
    let WorkerHoldings {
        inflight,
        inflight_members,
        receiver,
        pending,
    } = holdings;
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    // Members still in the receiver were never authorized and never handed to a
    // transport, so nothing about resolving them depends on whether the old
    // generation ceased. Resolving them first makes the specified
    // `dropped_on_shutdown` guarantee independent of how long the fence takes —
    // which is the whole of the defect this ordering fixes, since a fence that
    // outlived the process budget used to take these members down with it.
    //
    // Nothing can arrive after this point: `close_worker` has already returned,
    // and `try_existing_worker` holds the registry lock across its send, so a
    // sender either enqueued before the close (and is drained here) or observed
    // the entry closing and never enqueued at all.
    super::super::super::async_worker::drop_pending_async_tasks_on_stop(
        receiver,
        pending,
        cause.guard_trigger(),
    );
    // One window, not the whole remaining grace: the fence spends *two* of these
    // back to back, so a window sized at everything left would overrun the
    // deadline by almost that much again. The reserve covers what still has to
    // happen after the verdict — terminalizing whatever the collectors did not
    // resolve, unregistering, and the caller's own teardown.
    let fence_observation = budget_within_shutdown(
        fence_observation,
        Duration::from_millis(SHUTDOWN_FENCE_RESERVE_MS),
    ) / 2;
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
    emit_fence_verdict(key, cause.fence_trigger(), outcome, inflight_members.len());
    if outcome.verdict == FenceVerdict::Positive {
        transport.shutdown();
    }
    // Any member still in the table was never joined — its collector neither
    // resolved nor panicked, so the drain left it unresolved. Terminalize it
    // through the guard rather than dropping it: an ending is a trigger like any
    // other, and the evidence order still knows whether it reached a transport.
    for (_, member) in inflight_members.drain() {
        super::super::super::async_worker::complete_task_outcome_from_trigger(
            &member.task,
            cause.guard_trigger(),
        );
    }
    // Defensive only. Nothing can have arrived since the pre-fence drain above,
    // for the reason given there; this costs one `try_recv` and means a future
    // change that reopens a producer during shutdown loses nothing silently.
    super::super::super::async_worker::drop_pending_async_tasks_on_stop(
        receiver,
        pending,
        cause.guard_trigger(),
    );
}
