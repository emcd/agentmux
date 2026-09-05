//! The loop that drives one target's generations and fills its mailbox.
//!
//! Under the pull model this worker delivers nothing. What it owns is everything
//! around delivery: it holds the target's `TransportImpl` for the target's
//! lifetime, takes each received task into custody, supervises the generation
//! through the execution watchdog and the fence, and gives the target up when it
//! ends. The writing is the transport's own delivery-loop executor, which
//! consumes the mailbox this loop fills.

use std::time::{Duration, Instant};

use serde_json::json;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::relay::AsyncDeliveryTask;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;

use super::super::super::async_worker::{AsyncWorkerKey, WorkerOwner};
use super::super::super::fence::{FenceInProgress, FenceVerdict};
use super::super::super::guard::GuardTrigger;
use super::intake::take_into_mailbox;
use super::spawn::{
    ASYNC_WORKER_POLL_INTERVAL_MS, BuiltGeneration, ConsumerGenerationStep, WorkerTransportSource,
    build_generation,
};
use super::stop::{
    StopCause, abandon_target, emit_fence_verdict, resolve_queued_tasks_and_reclaim,
    shutdown_before_generation, stop_drain,
};

pub(super) async fn run_async_delivery_worker(
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
    // whichever task happens to arrive first.
    //
    // The loop below never writes. It receives, takes custody, and supervises;
    // the transport's own executor peeks what this put in the mailbox and decides
    // when to write it. That is the whole of the inversion at this site: what was
    // a produce-and-collect loop is a produce loop, because there is nothing left
    // for the relay to collect.
    let batch_settings = super::super::prompt_batch_settings();
    // Subscribed before any level is read, and before the transport that will
    // poke it exists. The contract requires subscribe-before-check precisely so
    // a change occurring between the check and the subscription cannot be missed;
    // creating this first makes that ordering unrepresentable rather than merely
    // observed.
    //
    // It no longer wakes a decision. What waits for a target to become ready is
    // the transport's own executor, on its own doorbell; this survives because the
    // tmux observer captures it at `startup` and would otherwise have no wakeup
    // path at all.
    let readiness_changed = std::sync::Arc::new(tokio::sync::Notify::new());
    // Observed before anything is constructed, not only inside the loop below.
    // Constructing first would start a target for a relay on its way out — a Tmux
    // pane or a Pty child spawned for a generation that can only be torn down
    // again, off-runtime and possibly blocking, delaying the exit it cannot serve.
    // The same invariant is why `initialize_acp_target_for_startup` checks
    // shutdown before creating a worker at all rather than only while waiting on
    // one.
    if shutdown_requested() {
        shutdown_before_generation(&key, owner, &mut receiver, pending.as_ref());
        return;
    }
    // The first generation. An ACP worker starts its bootstrap here but does not
    // wait on it: awaiting made the loop below — and therefore this worker's
    // shutdown gate — unreachable for as long as the bootstrap took, which for an
    // agent parked in its `initialize` handshake is forever.
    let BuiltGeneration {
        mut transport,
        // Held so the next generation can name this one as outgoing, and so the
        // worker keeps one place that answers "who consumes this target now".
        mut binding,
        ..
    } = match build_generation(
        &key,
        owner,
        &source,
        batch_settings,
        &readiness_changed,
        ConsumerGenerationStep::Claim,
    )
    .await
    {
        Ok(built) => built,
        Err(error) => {
            // Construction failed, so this worker has no transport and never
            // will. Resolve everything it was elected to carry rather than
            // installing a dead transport whose executor would peek a mailbox it
            // could not write. Unregistering — rather than fail-stopping — is
            // deliberate: no generation ever started, so there is nothing a
            // replacement could race, and the next send is free to try again.
            //
            // Unregistering happens BEFORE the drain, and the order is the whole
            // of its correctness. `try_existing_worker` holds the registry lock
            // across `sender.send`, so once the entry is gone no send can still be
            // in flight — whereas draining first leaves the entry accepting, and a
            // send landing between the drain observing empty and the unregister is
            // lost with the receiver: no terminal outcome, no quota release. That
            // is the accept-after-drain race `close_worker` exists to prevent on
            // the shutdown path, and this path has to close the same way.
            super::super::super::async_worker::unregister_worker(&key, owner);
            resolve_queued_tasks_and_reclaim(&key, &mut receiver, pending.as_ref(), &error);
            return;
        }
    };
    let poll_interval = Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS);
    let mut senders_dropped = false;
    // Graceful shutdown and the execution watchdog are what fence a generation.
    // Both bounds are the relay's own `[delivery]` settings, read once per worker.
    let delivery = super::super::super::admission::delivery_configuration();
    let fence_observation = Duration::from_millis(delivery.fence_observation_timeout_ms);
    let submission_timeout = Duration::from_millis(delivery.submission_timeout_ms);
    // The execution watchdog's fence, present only while one is in progress.
    let mut watchdog: Option<FenceInProgress> = None;
    // Set by a negative verdict and never cleared. From then on this worker keeps
    // its registry entry so no replacement generation can be elected, and stays
    // alive only to observe shutdown.
    let mut fail_stopped = false;

    loop {
        // Shutdown is checked first: when the relay is exiting, that is the true
        // cause even if this worker's bundle was also being torn down, and the
        // shutdown spelling is the one its members' senders are owed.
        let stop_cause = if shutdown_requested() {
            Some(StopCause::Shutdown)
        } else if super::super::super::async_worker::worker_stop_requested(&key) {
            Some(StopCause::BundleStop)
        } else {
            None
        };
        if let Some(cause) = stop_cause {
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
            super::super::super::async_worker::close_worker(&key, owner);
            stop_drain(
                &key,
                &binding,
                &mut transport,
                &mut receiver,
                pending.as_ref(),
                fence_observation,
                cause,
            )
            .await;
            break;
        }
        // The execution watchdog. Anchored at the declaration the relay recorded,
        // which is a relay-observed event rather than the inferred "write begins"
        // point the push model had to guess at: a unit still outstanding past the
        // bound means the executor supervising it has run longer than it is
        // allowed to. That is a statement about the relay's own supervised code,
        // not about the target. Nothing is terminalized here — the fence verdict
        // is the single resolution cut, and an acknowledgment arriving during the
        // observation windows still resolves its own members.
        if watchdog.is_none()
            && !fail_stopped
            && super::super::super::admission::declaration_age(&binding)
                .is_some_and(|age| age >= submission_timeout)
        {
            emit_inscription(
                "relay.delivery.watchdog.armed",
                &json!({
                    "namespace": key.namespace,
                    "target_session": key.target_session,
                    "submission_timeout_ms": submission_timeout.as_millis() as u64,
                }),
            );
            watchdog = Some(FenceInProgress::begin(&mut transport, fence_observation));
        }
        // Advanced before the wait rather than inside it, so an acknowledgment
        // keeps landing through both observation windows: the fence's whole point
        // is that a unit resolving in time reports its own evidence.
        let watchdog_verdict = watchdog
            .as_mut()
            .and_then(|fence| fence.advance(&mut transport, Instant::now()));
        if let Some(outcome) = watchdog_verdict {
            watchdog = None;
            emit_fence_verdict(&key, "submission_timeout", outcome);
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
                    // transport kind by being fenced. Whatever the outgoing
                    // generation had declared and not acknowledged is resolved as
                    // part of taking the target over, and reported there.
                    match build_generation(
                        &key,
                        owner,
                        &source,
                        batch_settings,
                        &readiness_changed,
                        ConsumerGenerationStep::Replace {
                            outgoing: binding.generation,
                        },
                    )
                    .await
                    {
                        Ok(built) => {
                            transport = built.transport;
                            binding = built.binding;
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
                            super::super::super::async_worker::unregister_worker(&key, owner);
                            abandon_target(&binding, GuardTrigger::ExecutionBound);
                            resolve_queued_tasks_and_reclaim(
                                &key,
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
                    super::super::super::async_worker::mark_worker_fail_stopped(&key, owner);
                    fail_stopped = true;
                    // Nothing will ever peek this mailbox again, so everything in
                    // it is owed an answer — including entries whose target is
                    // perfectly reachable. That is the one deliberate exception to
                    // "no elapsed duration resolves a reachable target's entry",
                    // and it is not really elapsed duration: the finding is that
                    // no consumer can be elected for this target, and a member
                    // held behind that can never be delivered by anything.
                    abandon_target(&binding, GuardTrigger::ExecutionBound);
                }
            }
        }

        tokio::select! {
            maybe_task = receiver.recv(), if !senders_dropped => {
                match maybe_task {
                    Some(task) => {
                        if shutdown_requested() {
                            super::super::super::async_worker::complete_task_on_shutdown(&task);
                            super::super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        if fail_stopped {
                            // Raced the registry mark: this task was handed over
                            // between the fail-stop check and the send. It was
                            // never declared, so the evidence order proves
                            // non-delivery rather than inferring it.
                            super::super::super::async_worker::complete_task_outcome_from_trigger(
                                &task,
                                GuardTrigger::ExecutionBound,
                            );
                            super::super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        // Custody is taken here: the entry's payload is built and
                        // placed in its target's mailbox. This is the first point
                        // the relay holds the task and still before any transport
                        // is contacted, and it is the last point this loop is
                        // involved in the delivery at all.
                        //
                        // A target whose bootstrap has not settled is filled all
                        // the same. An entry admitted now is one its executor
                        // peeks the moment its runtime arrives, and withholding it
                        // here would only delay it — where under the push model
                        // the same wait had to gate a handover this loop was about
                        // to perform.
                        take_into_mailbox(task, &transport, pending.as_ref());
                    }
                    None => senders_dropped = true,
                }
            }
            () = readiness_changed.notified() => {
                // A transport reported that its observed level moved. Nothing here
                // acts on it any more — the executor reads its own target — but
                // the notifier is still what the tmux observer fires, and
                // consuming it keeps its permits from accumulating.
            }
            () = tokio::time::sleep(poll_interval) => {
                // Poll tick: re-evaluate the shutdown gate and the watchdog even
                // while idle.
            }
        }
    }
    super::super::super::async_worker::unregister_worker(&key, owner);
}
