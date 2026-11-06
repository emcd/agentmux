#[cfg(test)]
#[cfg(all(test, feature = "pty"))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc as tokio_mpsc;

use serde_json::json;

use crate::configuration::{BundleMember, TargetConfiguration};

use super::super::super::{AsyncDeliveryTask, RelayError};
use crate::acp::state::ACP_STARTUP_PRIME_TIMEOUT_MS;

#[cfg(feature = "pty")]
use super::super::async_worker::{
    PtyElection, adopt_registered_attempt, elect_pty_attempt,
    worker_attempt_snapshot,
};
use super::super::async_worker::{
    WorkerDispatch, get_worker_failure, get_worker_readiness,
};
#[cfg(feature = "pty")]
use super::worker::PtyWorkerBootstrap;
use super::worker::{
    AcpWorkerBootstrap, WorkerTransportContext, WorkerTransportSource,
    spawn_async_delivery_worker,
};
use crate::relay::delivery::DeliveryContext;
use crate::runtime::signals::shutdown_requested;
use crate::transports::{WorkerFailureReason, WorkerReadinessState};

pub(in crate::relay) fn wait_for_async_delivery_shutdown(
    delivery: &Arc<DeliveryContext>,
    timeout: Duration,
) -> usize {
    super::super::async_worker::wait_for_async_delivery_shutdown(
        delivery, timeout,
    )
}

pub(in crate::relay) fn initialize_acp_target_for_startup(
    delivery: &Arc<DeliveryContext>,
    namespace: &str,
    runtime_directory: &std::path::Path,
    target_member: &BundleMember,
    choices_pending_max: usize,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    if !matches!(target_member.target, TargetConfiguration::Acp(_)) {
        return Ok(());
    }
    // Checked before the worker is created, not only while waiting on it:
    // starting a target means spawning an agent process, and a relay on its way
    // out has no use for one. Signalled part-way through a bundle, this is the
    // difference between the remaining members never starting and each of them
    // starting an agent for a fence to have to end.
    if shutdown_requested() || delivery.is_shutting_down() {
        return Err(startup_interrupted_error(
            target_member.id.as_str(),
            "ACP",
        ));
    }
    if target_member.working_directory.is_none() {
        return Err((
            "runtime_acp_initialize_failed".to_string(),
            "ACP startup requires target working directory".to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    let key = super::super::async_worker::build_worker_key(
        namespace,
        runtime_directory,
        target_member.id.as_str(),
    );
    // A worker on its way out must never be adopted as this target's live one.
    // The election below is by key presence, so a draining entry would be found,
    // the spawn skipped, and the readiness poll answered from the *previous*
    // generation's last reported state — a member counted ready on a worker that
    // is about to unregister, under the configuration this pass was meant to
    // replace. Reported as a failure rather than waited out: the member genuinely
    // did not start, saying so is honest, and the next pass starts it once the
    // key is free. Reached whenever a bring-up follows a teardown closely enough
    // that a drain outlives the teardown's bounded wait — a watcher reload, or an
    // operator's `down` followed by `up`.
    if super::super::async_worker::worker_stop_requested(
        delivery.async_registry(),
        &key,
    ) {
        return Err((
            "runtime_startup_failed".to_string(),
            "ACP worker from the previous definition is still stopping"
                .to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    if !super::super::async_worker::worker_exists(
        delivery.async_registry(),
        &key,
    )
    .map_err(|error| {
        (
            "internal_unexpected_failure".to_string(),
            "failed to query ACP worker registry".to_string(),
            Some(json!({
                "target_session": target_member.id,
                "cause": error.message,
            })),
        )
    })? {
        let (sender, receiver) =
            tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let pending =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bootstrap = AcpWorkerBootstrap {
            target_member: target_member.clone(),
            runtime_directory: runtime_directory.to_path_buf(),
            choices_pending_max,
        };
        if let Some(owner) =
            super::super::async_worker::register_worker_if_absent(
                delivery.async_registry(),
                key.clone(),
                sender,
                pending.clone(),
                true,
            )
            .map_err(|error| {
                (
                    "internal_unexpected_failure".to_string(),
                    "failed to register ACP worker".to_string(),
                    Some(json!({
                        "target_session": target_member.id,
                        "cause": error.message,
                    })),
                )
            })?
        {
            spawn_async_delivery_worker(
                Arc::clone(delivery),
                key,
                owner,
                receiver,
                pending,
                WorkerTransportSource::Acp(bootstrap),
            );
        }
    }
    let deadline =
        Instant::now() + Duration::from_millis(ACP_STARTUP_PRIME_TIMEOUT_MS);
    loop {
        // A worker told to stop will not become ready, and it deregisters as it
        // drains — taking the readiness this poll reads with it, so the wait
        // would run its full timeout against an answer that can no longer
        // change. Signalled during a multi-member startup, that cost the relay
        // its whole timeout for every member still to come.
        if shutdown_requested() || delivery.is_shutting_down() {
            return Err(startup_interrupted_error(
                target_member.id.as_str(),
                "ACP",
            ));
        }
        let readiness = get_worker_readiness(
            delivery.async_registry(),
            namespace,
            runtime_directory,
            target_member.id.as_str(),
        );
        // The same acceptance set `list` reports, so a session this poll counts
        // ready is one `list` also calls ready.
        if super::super::async_worker::acp_readiness_is_ready(readiness) {
            return Ok(());
        }
        match readiness {
            Some(WorkerReadinessState::Unavailable) => {
                let recorded = get_worker_failure(
                    delivery.async_registry(),
                    namespace,
                    runtime_directory,
                    target_member.id.as_str(),
                );
                return Err(startup_failure_error(
                    recorded,
                    "runtime_acp_worker_unavailable",
                    "ACP worker is unavailable after startup",
                    json!({
                        "target_session": target_member.id,
                    }),
                ));
            },
            // Still settling. `Available`/`Busy` returned above, so this arm is
            // reached only for `Initializing`, `Recovering`, and an unregistered
            // worker.
            _ => {
                if Instant::now() >= deadline {
                    let recorded = get_worker_failure(
                        delivery.async_registry(),
                        namespace,
                        runtime_directory,
                        target_member.id.as_str(),
                    );
                    return Err(startup_failure_error(
                        recorded,
                        "runtime_startup_failed",
                        "ACP worker did not become ready during startup",
                        json!({
                            "target_session": target_member.id,
                            "timeout_ms": ACP_STARTUP_PRIME_TIMEOUT_MS,
                        }),
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            },
        }
    }
}

/// Bound on a Pty worker reaching a ready state during bring-up.
///
/// Child spawn is local and fast unlike the ACP agent handshake, so the
/// bound is tighter than `ACP_STARTUP_PRIME_TIMEOUT_MS`. It binds only
/// pathological slowness: construction failure surfaces through the
/// vanished-worker arm below without waiting out the bound.
#[cfg(feature = "pty")]
const PTY_STARTUP_PRIME_TIMEOUT_MS: u64 = 5_000;

// Test seam: the counter below lets prime-wait tests stage helper-thread
// actions strictly after the loop has started another pass (a counter
// advance proves the pre-election guard passed and the loop is polling),
// instead of guessing with sleeps. It counts loop passes only, not
// snapshots — a pass may observe absence — and is compiled out of
// non-test builds. Concurrent prime waits would advance it on a waiting
// test's behalf, so the waiting tests serialize on the shared lock below.
#[cfg(all(test, feature = "pty"))]
pub(super) static PRIME_WAIT_POLLS: AtomicUsize = AtomicUsize::new(0);

/// Eagerly elects a Pty target's delivery worker at bundle bring-up.
///
/// Mirrors `initialize_acp_target_for_startup`: the registry election is
/// the handoff, so whichever site wins — bring-up here or a first delivery
/// racing it — owns the target's only worker, and the child is spawned
/// inside that worker's first generation build (post-registration), which
/// is what makes a second child unrepresentable. A repeated pass over an
/// already-registered worker changes nothing, so startup and reconcile
/// agree about what starting a Pty member means.
#[cfg(feature = "pty")]
pub(in crate::relay) fn initialize_pty_target_for_startup(
    delivery: &Arc<DeliveryContext>,
    namespace: &str,
    runtime_directory: &std::path::Path,
    target_member: &BundleMember,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    if !matches!(target_member.target, TargetConfiguration::Pty(_)) {
        return Ok(());
    }
    // Checked before the worker is created, not only while waiting on it:
    // starting a target means spawning a child process, and a relay on its
    // way out has no use for one. The instance gate joins the process-wide
    // signal: a per-handle shutdown must refuse new children even while the
    // process itself keeps running.
    if shutdown_requested() || delivery.is_shutting_down() {
        return Err(startup_interrupted_error(
            target_member.id.as_str(),
            "Pty",
        ));
    }
    let key = super::super::async_worker::build_worker_key(
        namespace,
        runtime_directory,
        target_member.id.as_str(),
    );
    let registry = delivery.async_registry();
    // Same draining-entry guard as the ACP path: a worker on its way out
    // must never be adopted as this target's live one.
    if super::super::async_worker::worker_stop_requested(registry, &key) {
        return Err((
            "runtime_startup_failed".to_string(),
            "Pty worker from the previous definition is still stopping"
                .to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    // The waited attempt: the owner this pass elected, or the owner it
    // found already registered — acquired atomically with observing the
    // registry entry, and carrying the attempt's completion handle. The
    // handle (not a re-read owner id) is what closes the adoption lookup
    // race: a fast failure unregistering between election and first poll
    // cannot strand this waiter, because the handle outlives the entry.
    // The election itself names an observed winner with its handle, so
    // there is no second lookup for a winner to vanish through.
    let mut waited = adopt_registered_attempt(registry, &key);
    if waited.is_none() {
        let (sender, receiver) =
            tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let pending =
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bootstrap = PtyWorkerBootstrap {
            namespace: namespace.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            target_member: target_member.clone(),
        };
        match elect_pty_attempt(
            registry,
            key.clone(),
            sender,
            pending.clone(),
            // Unbounded: Pty workers take every accepted task into the
            // mailbox and hold the pending slot to the terminal ack, so
            // a bounded queue would impose the ACP-specific
            // ACP_PENDING_MAX cap and `runtime_acp_queue_full` errors
            // on a transport that never asked for them. The lazy
            // election passes false for the same reason; whichever
            // election wins must produce identical queue semantics.
            false,
        )
        .map_err(|error| {
            (
                "internal_unexpected_failure".to_string(),
                "failed to register Pty worker".to_string(),
                Some(json!({
                    "target_session": target_member.id,
                    "cause": error.message,
                })),
            )
        })? {
            PtyElection::Elected { owner, completion } => {
                // The handle is born with the election under one
                // acquisition — never fetched after it — so no purge can
                // land between naming the attempt and retaining its
                // result.
                spawn_async_delivery_worker(
                    Arc::clone(delivery),
                    key.clone(),
                    owner,
                    receiver,
                    pending,
                    WorkerTransportSource::Pty(bootstrap),
                );
                waited = Some((owner, completion));
            },
            // Already registered: the election observed the winner and
            // named it with its handle under the same acquisition, so no
            // second lookup — and no vanished-winner gap — stands between
            // this pass and the attempt it waits on. No spawn: the
            // winner's worker already exists.
            PtyElection::Existing { owner, completion } => {
                waited = Some((owner, completion));
            },
        }
    }
    // Prime wait: registration alone must not report ready. The waiter
    // holds the waited attempt's completion handle, so concurrent waiters
    // observe one immutable result, a replacement attempt's records can
    // never describe this one, and no purge or unregister can blind a
    // waiter that has not yet polled. (`acp_readiness_is_ready` is reused
    // for the serving predicate; the name is ACP-specific, the
    // `Available | Busy` acceptance is transport-neutral — the same set
    // `list` reports.)
    //
    // Poll order per pass: superseded attempt first (adopt the successor,
    // or fail the vanished one), then serving, then the attempt's recorded
    // cause, then the deadline. Serving is read from the same atomic
    // snapshot as the owner, so a serving observation is always attributed
    // to the worker that produced it — never another worker's
    // target-scoped readiness treated as the waited attempt's result.
    // Serving precedes the recorded cause on purpose: a worker that serves
    // is definitionally not described by a failure record. The transport's
    // own code is preferred only when attributable to the waited attempt;
    // every other exit carries honest phase details instead.
    let deadline =
        Instant::now() + Duration::from_millis(PTY_STARTUP_PRIME_TIMEOUT_MS);
    // A stop signalled after a valid ready observation is a later event;
    // an entry already stopping at a snapshot is never reported serving.
    loop {
        if shutdown_requested() || delivery.is_shutting_down() {
            return Err(startup_interrupted_error(
                target_member.id.as_str(),
                "Pty",
            ));
        }
        #[cfg(all(test, feature = "pty"))]
        PRIME_WAIT_POLLS.fetch_add(1, Ordering::Relaxed);
        // One acquisition covers owner, readiness, and stopping together:
        // check below and the supersession check above read the same
        // snapshot, so neither can describe a worker the other did not see.
        let snapshot = worker_attempt_snapshot(registry, &key);
        let current = snapshot.as_ref().map(|(owner, ..)| *owner);
        let waited_owner = waited.as_ref().map(|(owner, _)| *owner);
        if current != waited_owner {
            if let Some((owner, completion)) =
                adopt_registered_attempt(registry, &key)
            {
                // Superseded: another election replaced the waited attempt.
                // Adopt the successor — owner and handle together, under
                // one acquisition — rather than reporting the old
                // attempt's outcome for the member's current state.
                waited = Some((owner, completion));
                continue;
            }
            // No worker registered. If this pass elected or adopted an
            // attempt that has since vanished, its handle still names the
            // recorded cause: the record precedes the unregister in the
            // failing worker, and the handle outlives both. That is the
            // prompt signal, not the deadline — a failed construction must
            // not wait out the bound.
            if let Some((_, completion)) = waited.as_ref()
                && let Some(recorded) = completion.read()
            {
                return Err(startup_failure_error(
                    Some(recorded),
                    "runtime_pty_startup_failed",
                    "Pty worker unregistered during startup",
                    json!({
                        "target_session": target_member.id,
                    }),
                ));
            }
            // No waited attempt and none registered. The election above
            // always names an attempt, so this arm is defensive rather
            // than a semantics the election relies on: keep polling for
            // a successor until the deadline rather than inventing a
            // cause — the phase detail names what happened instead of a
            // generic timeout.
            if waited.is_none() {
                if Instant::now() >= deadline {
                    return Err(startup_failure_error(
                        None,
                        "runtime_startup_failed",
                        "Pty worker vanished before adoption during startup",
                        json!({
                            "target_session": target_member.id,
                            "timeout_ms": PTY_STARTUP_PRIME_TIMEOUT_MS,
                        }),
                    ));
                }
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            return Err(startup_failure_error(
                waited
                    .as_ref()
                    .and_then(|(_, completion)| completion.read()),
                "runtime_pty_startup_failed",
                "Pty worker unregistered during startup",
                json!({
                    "target_session": target_member.id,
                }),
            ));
        }
        let readiness = snapshot.and_then(|(_, readiness, _)| readiness);
        let stopping = snapshot
            .as_ref()
            .map(|(_, _, stopping)| *stopping)
            .unwrap_or(false);
        if super::super::async_worker::acp_readiness_is_ready(readiness)
            && !stopping
        {
            // A serving worker is definitionally not described by a failure
            // record, and a new entry carries a fresh handle anyway: there
            // is no stale state to clear. Eligibility was rechecked on this
            // same snapshot, so a stopping entry is never reported serving
            // no matter when the stop landed relative to this poll.
            return Ok(());
        }
        // An Unavailable observation for the waited attempt ends the wait.
        // A recorded cause wins: construction records before it publishes
        // the terminal registry state, so a cause still in flight at the
        // moment of observation is unrepresentable — the registry cannot
        // show this attempt as Unavailable until its record has landed.
        // With no cause, the state itself is the report: a genuinely
        // causeless terminal state, never a generic timeout.
        if readiness == Some(WorkerReadinessState::Unavailable) {
            if let Some((_, completion)) = waited.as_ref()
                && let Some(recorded) = completion.read()
            {
                return Err(startup_failure_error(
                    Some(recorded),
                    "runtime_pty_startup_failed",
                    "Pty worker reported Unavailable during startup",
                    json!({
                        "target_session": target_member.id,
                    }),
                ));
            }
            return Err(startup_failure_error(
                None,
                "runtime_pty_unavailable",
                "Pty worker reported Unavailable during startup",
                json!({
                    "target_session": target_member.id,
                    "last_readiness": format!("{readiness:?}"),
                }),
            ));
        }
        if let Some((_, completion)) = waited.as_ref()
            && let Some(recorded) = completion.read()
        {
            return Err(startup_failure_error(
                Some(recorded),
                "runtime_pty_startup_failed",
                "Pty worker unregistered during startup",
                json!({
                    "target_session": target_member.id,
                }),
            ));
        }
        if Instant::now() >= deadline {
            if readiness == Some(WorkerReadinessState::Unavailable) {
                return Err(startup_failure_error(
                    None,
                    "runtime_pty_unavailable",
                    "Pty worker reported Unavailable during startup",
                    json!({
                        "target_session": target_member.id,
                        "timeout_ms": PTY_STARTUP_PRIME_TIMEOUT_MS,
                        "last_readiness": format!("{readiness:?}"),
                    }),
                ));
            }
            return Err(startup_failure_error(
                None,
                "runtime_startup_failed",
                "Pty worker did not become ready during startup",
                json!({
                    "target_session": target_member.id,
                    "timeout_ms": PTY_STARTUP_PRIME_TIMEOUT_MS,
                    "last_readiness": format!("{readiness:?}"),
                }),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Reports a startup abandoned because the relay is shutting down. The
/// transport name keeps the phase detail honest: a Pty bring-up refused by
/// the gate must not report an ACP startup.
fn startup_interrupted_error(
    target_session: &str,
    transport: &str,
) -> (String, String, Option<serde_json::Value>) {
    (
        "runtime_startup_interrupted".to_string(),
        format!("relay shutdown requested during {transport} worker startup"),
        Some(json!({
            "target_session": target_session,
        })),
    )
}

/// Builds the startup failure tuple, preferring the worker's recorded structured
/// failure — the true cause captured inside the worker task (e.g. why the ACP
/// `initialize` handshake failed) — over the generic `fallback_*` placeholder,
/// which only names how the readiness poll concluded (unavailable, or timed out
/// still initializing). When a recorded failure is present its code/reason
/// replace the placeholder; the startup-path `details` (target session, timeout)
/// are retained either way so the operator keeps that context.
fn startup_failure_error(
    recorded: Option<WorkerFailureReason>,
    fallback_code: &str,
    fallback_reason: &str,
    details: serde_json::Value,
) -> (String, String, Option<serde_json::Value>) {
    match recorded {
        Some(failure) => (failure.code, failure.reason, Some(details)),
        None => (
            fallback_code.to_string(),
            fallback_reason.to_string(),
            Some(details),
        ),
    }
}

pub(in crate::relay) fn enqueue_async_delivery(
    delivery: &Arc<DeliveryContext>,
    task: AsyncDeliveryTask,
) -> Result<(), RelayError> {
    enqueue_delivery_task(delivery, task)
}

fn enqueue_delivery_task(
    delivery: &Arc<DeliveryContext>,
    task: AsyncDeliveryTask,
) -> Result<(), RelayError> {
    let bounded_acp_queue =
        super::super::async_worker::task_uses_acp_transport(&task);
    let key = super::super::async_worker::build_worker_key(
        task.bundle.bundle_name.as_str(),
        task.runtime_directory.as_path(),
        task.target_session.as_str(),
    );
    if bounded_acp_queue
        && !super::super::async_worker::worker_exists(
            delivery.async_registry(),
            &key,
        )?
    {
        return Err(super::super::super::relay_error(
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": task.target_session,
            })),
        ));
    }
    match super::super::async_worker::try_existing_worker(
        delivery.async_registry(),
        &key,
        task,
    )? {
        WorkerDispatch::Accepted => Ok(()),
        WorkerDispatch::Closing(task) => {
            // The target's worker is draining for relay shutdown. Record the drop
            // on the observability floor and stop; spawning here would resurrect a
            // worker mid-shutdown and clobber the closing registry entry the
            // shutdown barrier still counts (an accept-after-drain regression).
            super::super::async_worker::complete_task_on_shutdown(
                delivery, &task,
            );
            Ok(())
        },
        // Reported as an error rather than resolved as a delivery outcome: nothing
        // was admitted for this task yet, and the condition is about the target
        // rather than about this message. The sender is told the target is
        // fail-stopped so it stops sending, instead of collecting one
        // indistinguishable non-delivery receipt per attempt.
        WorkerDispatch::FailStopped(task) => Err(fail_stopped_error(&task)),
        // A worker that was never there and one whose receiver has since dropped
        // are the same instruction to a spawner: there is no live worker, make
        // one. They differ only for an observer counting closed notification
        // paths, which is not this path's concern.
        WorkerDispatch::Missing(task) | WorkerDispatch::Dropped(task) => {
            // ACP workers are pre-created during startup and never lazily created
            // here. Reaching the new-worker path with an ACP task means its worker
            // has gone away, so report it unavailable rather than spin up a
            // non-bootstrap worker. This guard makes the rest of this arm provably
            // non-ACP, so the worker created below is always unbounded.
            if bounded_acp_queue {
                return Err(super::super::super::relay_error(
                    "runtime_acp_worker_unavailable",
                    "ACP worker is unavailable for target session",
                    Some(json!({
                        "target_session": task.target_session,
                    })),
                ));
            }
            // Resolved before anything is registered, from the task electing this
            // worker. The transport kind is a property of the target rather than
            // of this message, so resolving it here is what lets the worker build
            // once at spawn — and it means a target whose member cannot be
            // resolved is reported to this sender synchronously instead of being
            // handed to a worker that would discover it later. Registering first
            // would leave an entry with a live sender and no worker behind it.
            let transport_context =
                WorkerTransportContext::resolve(delivery, &task)?;
            let (sender, receiver) =
                tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
            let pending =
                std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            // The registry insert is the election, and it happens before anything
            // is spawned. Two concurrent first sends both reach this arm; only the
            // one that installs an entry owns the target, and only it starts a
            // worker. Spawning first and registering after let both start, let the
            // second registration orphan the first — which still held and executed
            // a task — and then let that orphan's exit delete the survivor's entry.
            match super::super::async_worker::register_worker_if_absent(
                delivery.async_registry(),
                key.clone(),
                sender.clone(),
                pending.clone(),
                // Unbounded: this arm elects workers for every non-ACP
                // transport (Direct dispatch included), so the ACP-only
                // pending cap must not apply here — only the ACP startup
                // election above bounds its queue.
                false,
            )? {
                Some(owner) => {
                    sender.send(task).map_err(|source| {
                        super::super::super::relay_error(
                            "internal_unexpected_failure",
                            "failed to enqueue async delivery task",
                            Some(json!({"cause": source.to_string()})),
                        )
                    })?;
                    spawn_async_delivery_worker(
                        Arc::clone(delivery),
                        key,
                        owner,
                        receiver,
                        pending,
                        WorkerTransportSource::Direct(transport_context),
                    );
                    Ok(())
                },
                // Lost the election: someone installed between the lookup above and
                // this claim. Hand the task to the owner they installed rather than
                // to the channel nothing will ever read.
                None => dispatch_to_installed_owner(delivery, &key, task),
            }
        },
    }
}

/// Dispatches a task to whichever worker won the registration race.
///
/// Deliberately not a retry loop. Reaching `Missing` here means the owner
/// installed and vanished inside this window, which is not a condition the relay
/// can resolve by trying again — reporting it names the state instead of
/// spinning.
fn dispatch_to_installed_owner(
    delivery: &Arc<DeliveryContext>,
    key: &super::super::async_worker::AsyncWorkerKey,
    task: AsyncDeliveryTask,
) -> Result<(), RelayError> {
    match super::super::async_worker::try_existing_worker(
        delivery.async_registry(),
        key,
        task,
    )? {
        WorkerDispatch::Accepted => Ok(()),
        WorkerDispatch::Closing(task) => {
            super::super::async_worker::complete_task_on_shutdown(
                delivery, &task,
            );
            Ok(())
        },
        WorkerDispatch::FailStopped(task) => Err(fail_stopped_error(&task)),
        WorkerDispatch::Missing(task) | WorkerDispatch::Dropped(task) => {
            Err(super::super::super::relay_error(
                "internal_unexpected_failure",
                "target worker was replaced while the delivery was being enqueued",
                Some(json!({"target_session": task.target_session})),
            ))
        },
    }
}

/// The error a fail-stopped target refuses every send with.
///
/// It names the condition rather than the message, because the message is not
/// what went wrong: the relay could not establish that a previous generation had
/// stopped writing to this target, and starting a second one alongside it is the
/// hazard the fence exists to avoid.
fn fail_stopped_error(task: &AsyncDeliveryTask) -> RelayError {
    super::super::super::relay_error(
        "delivery_target_fail_stopped",
        "target is fail-stopped: a previous generation was not observed to cease",
        Some(json!({"target_session": task.target_session})),
    )
}

#[cfg(test)]
mod tests;
