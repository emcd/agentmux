//! Ending a worker: the pre-generation exit, the fence-bounded drain, and the
//! records both leave behind.

use std::time::{Duration, Instant};

use serde_json::json;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::protocol::identity::{ConsumerBinding, DeliveryTargetId};
use crate::relay::delivery::admission::{ResolvedMember, reap_target, resolve_target_entries};
use crate::relay::{AsyncDeliveryTask, RelayError, SendOutcome, SendResult};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::budget_within_shutdown;
use crate::transports::TransportImpl;

use super::super::super::async_worker::{
    AsyncWorkerKey, WorkerOwner, report_resolved_member, report_resolved_outcome,
};
use super::super::super::fence::{FenceInProgress, FenceOutcome, FenceResolution, FenceVerdict};
use super::super::super::guard::GuardTrigger;
use super::spawn::{ASYNC_WORKER_POLL_INTERVAL_MS, SHUTDOWN_FENCE_RESERVE_MS};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

/// Reports an undeclared entry the relay shut down before anything wrote it.
///
/// The one place a lifecycle event names its own outcome rather than taking the
/// guard's, and the contract admits it for exactly this shape: an undeclared
/// entry is provably unwritten, so `dropped_on_shutdown` is both true and more
/// informative than the `not_submitted` the evidence order would otherwise give.
fn complete_resolved_on_shutdown(member: &ResolvedMember) {
    report_resolved_outcome(
        member,
        SendResult {
            target_session: member.task.target_session.clone(),
            message_id: member.message_id.clone(),
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            details: None,
        },
    );
}

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
/// the construction failure that made it so, and then reclaims the target its
/// unregister could not.
///
/// Used only where the worker is about to end without a transport. The tasks were
/// admitted but never authorized, so they are terminalized with the real cause
/// rather than left in a receiver nothing will poll.
///
/// **The two halves are one function because every caller owes both.** Both
/// construction-failure paths unregister *before* draining — holding the
/// registry lock across the send is what keeps a task out of a receiver nothing
/// will poll again — so the reap that rode their unregister found these entries
/// still admitted and kept the mailbox rather than resolving entries it could
/// not report. Reclaiming is therefore owed once the drain has resolved them,
/// and pairing the two here is what keeps a third such path from inheriting the
/// obligation without inheriting the code: splitting them left one of the two
/// existing callers without the retry, which is how this pairing came about.
///
/// The retry names no generation. The unregister's reap already gave the
/// target's generation up, so naming the one this worker held would be refused
/// by the target it just released; naming nothing matches a target nobody has
/// claimed and is refused by one somebody claimed in the meantime, which is the
/// answer that case wants.
pub(super) fn resolve_queued_tasks_and_reclaim(
    key: &AsyncWorkerKey,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
    error: &RelayError,
) {
    while let Ok(task) = receiver.try_recv() {
        super::super::super::async_worker::complete_task_outcome(&task, Err(error.clone()));
        super::super::super::async_worker::release_pending_slot(pending);
    }
    reap_target(
        &DeliveryTargetId::new(
            key.namespace.as_str(),
            key.runtime_directory.as_path(),
            key.target_session.as_str(),
        ),
        None,
    )
    .ok();
}

/// Records one fence verdict, naming what triggered the fence.
///
/// Shared by graceful shutdown and the execution watchdog because the fence is
/// one protocol with one vocabulary: an operator reading these should be able to
/// compare a shutdown verdict against a watchdog verdict without translating.
pub(super) fn emit_fence_verdict(key: &AsyncWorkerKey, trigger: &str, outcome: FenceOutcome) {
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
        }),
    );
}

/// Resolves everything a target's mailbox still holds, because nothing will ever
/// peek it again.
///
/// The two callers are the two findings that mean exactly that: a fail-stopped
/// generation, for which no replacement may be elected, and a worker ending
/// without one. Each entry resolves by the route its declaration state calls for
/// — the ledger decides that, not this — and `trigger` supplies only the reason a
/// sender is told.
///
/// Members already resolved are not reported twice: the terminal transition is
/// contested, and only what this call wins comes back.
pub(super) fn abandon_target(binding: &ConsumerBinding, trigger: GuardTrigger) {
    for member in &resolve_target_entries(binding) {
        report_resolved_member(member, None, Some(trigger.reason()));
    }
}

/// Resolves everything a target's mailbox holds at graceful shutdown or a bundle
/// stop, spelling each member's outcome by whether it had been declared.
///
/// **An undeclared entry resolves `dropped_on_shutdown` and a declared one must
/// not.** Nothing was ever about to write the first, so naming the process
/// exiting is both true and more use to its sender than the generic
/// non-delivery; a write may already have reached the target for the second, so
/// it takes the guard's evidence order — `submission_unknown` where no
/// acknowledgment arrived. Collapsing the two would let a lifecycle event choose
/// an outcome, which is precisely what the evidence order exists to prevent.
fn resolve_mailbox_at_stop(binding: &ConsumerBinding, cause: StopCause) {
    for member in &resolve_target_entries(binding) {
        if member.declared || cause != StopCause::Shutdown {
            report_resolved_member(member, None, Some(cause.guard_trigger().reason()));
        } else {
            complete_resolved_on_shutdown(member);
        }
    }
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
    binding: &ConsumerBinding,
    transport: &mut TransportImpl,
    receiver: &mut UnboundedReceiver<AsyncDeliveryTask>,
    pending: &std::sync::atomic::AtomicUsize,
    fence_observation: Duration,
    cause: StopCause,
) {
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    // Tasks still in the receiver never reached a mailbox, so nothing about
    // resolving them depends on whether the old generation ceased. Resolving them
    // first makes the specified `dropped_on_shutdown` guarantee independent of how
    // long the fence takes — which is the whole of the defect this ordering fixes,
    // since a fence that outlived the process budget used to take these members
    // down with it.
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
    // happen after the verdict — resolving whatever the executor did not
    // acknowledge, unregistering, and the caller's own teardown.
    let fence_observation = budget_within_shutdown(
        fence_observation,
        Duration::from_millis(SHUTDOWN_FENCE_RESERVE_MS),
    ) / 2;
    let mut fence = FenceInProgress::begin(transport, fence_observation);
    let outcome = loop {
        if let Some(outcome) = fence.advance(transport, Instant::now()) {
            break outcome;
        }
        // The executor keeps acknowledging through both windows. That is the
        // whole reason the fence is stepped rather than awaited: a unit that
        // settles in time reports its own evidence, and a caller that blocked
        // here would stop collecting exactly the outcomes the fence exists to let
        // it keep collecting.
        tokio::time::sleep(poll_interval).await;
    };
    emit_fence_verdict(key, cause.fence_trigger(), outcome);
    if outcome.verdict == FenceVerdict::Positive {
        transport.shutdown();
    }
    // The verdict is the resolution cut. Whatever the executor left in the
    // mailbox is resolved here, by the route each entry's declaration state calls
    // for — and after the teardown above, so an acknowledgment that arrived
    // during the observation windows has already taken its own members and this
    // finds only what genuinely went unresolved.
    resolve_mailbox_at_stop(binding, cause);
    // Defensive only. Nothing can have arrived since the pre-fence drain above,
    // for the reason given there; this costs one `try_recv` and means a future
    // change that reopens a producer during shutdown loses nothing silently.
    super::super::super::async_worker::drop_pending_async_tasks_on_stop(
        receiver,
        pending,
        cause.guard_trigger(),
    );
}

/// A drain that resolves a worker's leftovers is also what makes its target
/// reclaimable.
///
/// Inline because the admission ledger is a process-global behind a
/// crate-private lock and this helper is `pub(super)` by design; reaching either
/// from `tests/` would publish them.
///
/// One test, driving the helper directly rather than through a worker whose
/// transport fails to build. What the worker adds above this call is the
/// decision to end, which both construction-failure paths already make
/// identically; what it cannot add is a third ordering, because the drain and
/// the reclaim are no longer separable. The lifecycle this pins is the one that
/// was actually wrong: a reap that ran while the queue was still admitted kept
/// the mailbox, and nothing afterwards took it.
#[cfg(test)]
mod reclaim_after_drain_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use crate::configuration::{
        BUNDLE_SCHEMA_VERSION, BundleConfiguration, BundleMember, SessionType, TargetConfiguration,
    };
    use crate::protocol::mailbox::MailboxPayload;
    use crate::relay::delivery::admission::{AdmissionTargetKey, TargetReap, admit, enqueue};
    use crate::transports::DeliveryPayloadMode;

    use super::*;

    const NAMESPACE: &str = "reclaim-after-drain-test";
    const TARGET: &str = "target";

    #[test]
    fn draining_a_queue_the_reap_had_to_keep_lets_the_retry_reclaim_it() {
        let runtime_directory = PathBuf::from("/nonexistent").join(NAMESPACE);
        let admission = AdmissionTargetKey::new(NAMESPACE, runtime_directory.as_path(), TARGET);
        let target = DeliveryTargetId::new(NAMESPACE, runtime_directory.as_path(), TARGET);
        let raw = || MailboxPayload::Raw {
            content: "body".to_string(),
            append_enter: true,
        };

        admit(
            "reclaim-after-drain-1",
            admission.clone(),
            SessionType::Tmux,
            1,
        )
        .expect("admit");
        enqueue(
            &Arc::new(queued_task(
                runtime_directory.as_path(),
                "reclaim-after-drain-1",
            )),
            raw(),
        )
        .expect("enqueue");

        // The reap that rides the unregister, which on these paths runs before
        // the drain. It finds the queued task still admitted and keeps the
        // mailbox rather than resolving an entry it could not report.
        assert_eq!(
            reap_target(&target, None),
            Ok(TargetReap::Retained { entries_held: 1 }),
            "the reap ahead of the drain keeps the mailbox it cannot empty"
        );

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(queued_task(
                runtime_directory.as_path(),
                "reclaim-after-drain-1",
            ))
            .expect("the receiver is live");
        drop(sender);
        let pending = std::sync::atomic::AtomicUsize::new(1);
        let error = crate::relay::relay_error("transport_unavailable", "no transport", None);
        resolve_queued_tasks_and_reclaim(
            &AsyncWorkerKey {
                runtime_directory: runtime_directory.clone(),
                namespace: NAMESPACE.to_string(),
                target_session: TARGET.to_string(),
            },
            &mut receiver,
            &pending,
            &error,
        );

        // The numbering is the discriminating observation, not emptiness. Had the
        // mailbox survived the retry, this entry would be position two in a
        // mailbox belonging to a target that no longer has a worker.
        admit("reclaim-after-drain-2", admission, SessionType::Tmux, 1).expect("admit");
        assert_eq!(
            enqueue(
                &Arc::new(queued_task(
                    runtime_directory.as_path(),
                    "reclaim-after-drain-2",
                )),
                raw(),
            )
            .expect("enqueue")
            .value(),
            1,
            "the retry after the drain reclaimed the mailbox, so numbering starts over"
        );
    }

    fn queued_task(runtime_directory: &Path, message_id: &str) -> AsyncDeliveryTask {
        AsyncDeliveryTask {
            admitted: true,
            bundle: BundleConfiguration {
                schema_version: BUNDLE_SCHEMA_VERSION.to_string(),
                bundle_name: NAMESPACE.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: NAMESPACE.to_string(),
            sender: BundleMember {
                id: "relay".to_string(),
                name: None,
                working_directory: None,
                target: TargetConfiguration::Ui,
                coder_session_id: None,
                policy_id: None,
                environment: Vec::new(),
            },
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: TARGET.to_string(),
            message: "body".to_string(),
            message_id: message_id.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: None,
        }
    }
}
