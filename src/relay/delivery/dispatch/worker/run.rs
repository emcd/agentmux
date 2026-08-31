//! The produce-and-collect loop that drives one worker's generations.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde_json::json;

use tokio::{sync::mpsc::UnboundedReceiver, task::JoinSet};

use crate::relay::AsyncDeliveryTask;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;

use super::super::super::async_worker::{AsyncWorkerKey, WorkerOwner};
use super::super::super::fence::{FenceInProgress, FenceVerdict};
use super::super::super::guard::GuardTrigger;
use super::super::batch::HandoverWindow;
use super::super::outcomes::collect_outcome;
use super::intake::{IntakeTask, take_into_mailbox};
use super::spawn::{
    ASYNC_WORKER_POLL_INTERVAL_MS, InflightMember, InflightOutcome, WorkerTransportSource,
    build_generation,
};
use super::stop::{
    StopCause, WorkerHoldings, emit_fence_verdict, resolve_queued_tasks_with_error,
    shutdown_before_generation, stop_drain, terminalize_unresolved_members,
};
use super::submit::{SubmitContext, submit_batch};

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
    let batch_settings = super::super::prompt_batch_settings();
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
                super::super::super::async_worker::unregister_worker(&key, owner);
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
    // One member received but not yet authorized — the head of the next batch.
    // It arrives here either straight off the channel, or because the last
    // attempt found its target unable to take a handover, or because it did not
    // fit the room the batch ahead of it left. In every case it stays queued —
    // quota reserved, nothing submitted, no batch minted — and is offered ahead
    // of anything newer so the target's FIFO order survives the wait. At most
    // one, and the receive arm is gated on its absence: taking a second member
    // off the channel while this one waits would reorder the target's queue.
    //
    // It carries the artifact its target is to be written, built and enqueued
    // when the task was received. The wait does not restamp or rebuild it: the
    // mailbox holds one payload per entry, and a member that waits out several
    // gate refusals must still be delivered the envelope the mailbox holds.
    let mut held: Option<IntakeTask> = None;
    // Graceful shutdown and the execution watchdog are what fence a generation.
    // Both bounds are the relay's own `[delivery]` settings, read once per worker.
    let delivery = super::super::super::admission::delivery_configuration();
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
    let unreachable_dwell = Duration::from_millis(delivery.unreachable_dwell_ms);
    // Batch formation's state: how much this target's transport is already
    // holding, against the maxima it declares. Built from the first generation
    // and never rebuilt, because the dimensions follow the target's session type
    // and a replacement generation is the same transport kind by contract.
    let mut window = HandoverWindow::for_transport(&transport);
    // Per worker, so it survives a generation replacement in the same loop. A
    // replacement's marker comes from the same tmux window, so the series stays
    // meaningful across one.
    let mut last_activity: Option<u64> = None;

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
            // A held member was never authorized, so the ending can say so
            // positively rather than leaving it to the guard's weaker inference.
            // Shutdown has its own spelling; a bundle stop goes through the guard,
            // whose evidence order reaches `not_submitted` for an unbound member
            // without needing a second wire outcome to mean the same thing.
            if let Some(member) = held.take() {
                match cause {
                    StopCause::Shutdown => {
                        super::super::super::async_worker::complete_task_on_shutdown(&member.task);
                    }
                    StopCause::BundleStop => {
                        super::super::super::async_worker::complete_task_outcome_from_trigger(
                            &member.task,
                            cause.guard_trigger(),
                        );
                    }
                }
                super::super::super::async_worker::release_pending_slot(pending.as_ref());
            }
            stop_drain(
                &key,
                &mut transport,
                WorkerHoldings {
                    inflight: &mut inflight,
                    inflight_members: &mut inflight_members,
                    receiver: &mut receiver,
                    pending: pending.as_ref(),
                },
                fence_observation,
                cause,
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
        // The handover window closes when the transport is holding nothing, and
        // only then: while any member of it is still in flight the transport is
        // still carrying that work, and admitting more would overrun the very
        // dimensions it declared. Closing is what restores a whole handover's
        // worth of room for the next member.
        if inflight.is_empty() {
            window.close();
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
                            super::super::super::async_worker::unregister_worker(&key, owner);
                            if let Some(member) = held.take() {
                                super::super::super::async_worker::complete_task_outcome(
                                    &member.task,
                                    Err(error.clone()),
                                );
                                super::super::super::async_worker::release_pending_slot(
                                    pending.as_ref(),
                                );
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
                    super::super::super::async_worker::mark_worker_fail_stopped(&key, owner);
                    fail_stopped = true;
                    if let Some(member) = held.take() {
                        super::super::super::async_worker::complete_task_outcome_from_trigger(
                            &member.task,
                            GuardTrigger::ExecutionBound,
                        );
                        super::super::super::async_worker::release_pending_slot(pending.as_ref());
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
        // The worker's only submission site. The receive arm below hands its task
        // here rather than submitting it, so that forming, authorizing and
        // submitting a batch is one path with one target gate at the head of it
        // rather than two callers that have to agree. Costs no latency — nothing
        // between that arm and here awaits.
        //
        // A member waiting on readiness retries here too. Its target may have
        // become ready since the last attempt; the poll arm below is what paces
        // the retries, and is the bounded backstop the readiness contract requires
        // so a missed notification only delays a delivery.
        if held.is_some() && bootstrap_settled_now && watchdog.is_none() && !fail_stopped {
            let head = held.take().expect("held member present in this branch");
            held = submit_batch(
                head,
                SubmitContext {
                    unreachable_dwell,
                    pending: pending.as_ref(),
                    window: &mut window,
                    last_activity: &mut last_activity,
                },
                &mut transport,
                &mut inflight,
                &mut inflight_members,
            )
            .await;
        }
        tokio::select! {
            maybe_task = receiver.recv(), if !senders_dropped && bootstrap_settled_now && held.is_none() && watchdog.is_none() && window.has_room() => {
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
                            // never authorized, so the evidence order proves
                            // non-delivery rather than inferring it.
                            super::super::super::async_worker::complete_task_outcome_from_trigger(
                                &task,
                                GuardTrigger::ExecutionBound,
                            );
                            super::super::super::async_worker::release_pending_slot(pending.as_ref());
                            continue;
                        }
                        // Received, not yet authorized: exactly what `held`
                        // means. The submission site at the top of the loop is
                        // what turns it into the head of a batch.
                        //
                        // Custody is taken here, which is where the entry's
                        // payload is built and placed in its target's mailbox.
                        // This is the first point the relay holds the task and
                        // still before any transport is contacted, so building
                        // here cannot depend on — or disturb — the gate below.
                        held = Some(take_into_mailbox(task, &transport));
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
    super::super::super::async_worker::unregister_worker(&key, owner);
}
