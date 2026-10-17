use std::{
    thread,
    time::{Duration, Instant},
};

use tokio::sync::mpsc as tokio_mpsc;

use serde_json::json;

use crate::configuration::{BundleMember, TargetConfiguration};

use super::super::super::{AsyncDeliveryTask, RelayError};
use crate::acp::state::ACP_STARTUP_PRIME_TIMEOUT_MS;

#[cfg(feature = "pty")]
use super::super::async_worker::take_construction_error;
use super::super::async_worker::{
    WorkerDispatch, get_worker_failure, get_worker_readiness,
};
#[cfg(feature = "pty")]
use super::worker::PtyWorkerBootstrap;
use super::worker::{
    AcpWorkerBootstrap, WorkerTransportContext, WorkerTransportSource,
    spawn_async_delivery_worker,
};
use crate::runtime::signals::shutdown_requested;
use crate::transports::{WorkerFailureReason, WorkerReadinessState};

pub(in crate::relay) fn wait_for_async_delivery_shutdown(
    timeout: Duration,
) -> usize {
    super::super::async_worker::wait_for_async_delivery_shutdown(timeout)
}

pub(in crate::relay) fn initialize_acp_target_for_startup(
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
    if shutdown_requested() {
        return Err(startup_interrupted_error(target_member.id.as_str()));
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
    if super::super::async_worker::worker_stop_requested(&key) {
        return Err((
            "runtime_startup_failed".to_string(),
            "ACP worker from the previous definition is still stopping"
                .to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    if !super::super::async_worker::worker_exists(&key).map_err(|error| {
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
        if shutdown_requested() {
            return Err(startup_interrupted_error(target_member.id.as_str()));
        }
        let readiness = get_worker_readiness(
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
    namespace: &str,
    runtime_directory: &std::path::Path,
    target_member: &BundleMember,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    if !matches!(target_member.target, TargetConfiguration::Pty(_)) {
        return Ok(());
    }
    // Checked before the worker is created, not only while waiting on it:
    // starting a target means spawning a child process, and a relay on its
    // way out has no use for one.
    if shutdown_requested() {
        return Err(startup_interrupted_error(target_member.id.as_str()));
    }
    let key = super::super::async_worker::build_worker_key(
        namespace,
        runtime_directory,
        target_member.id.as_str(),
    );
    // Same draining-entry guard as the ACP path: a worker on its way out
    // must never be adopted as this target's live one.
    if super::super::async_worker::worker_stop_requested(&key) {
        return Err((
            "runtime_startup_failed".to_string(),
            "Pty worker from the previous definition is still stopping"
                .to_string(),
            Some(json!({
                "target_session": target_member.id,
            })),
        ));
    }
    if !super::super::async_worker::worker_exists(&key).map_err(|error| {
        (
            "internal_unexpected_failure".to_string(),
            "failed to query Pty worker registry".to_string(),
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
        let bootstrap = PtyWorkerBootstrap {
            namespace: namespace.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            target_member: target_member.clone(),
        };
        if let Some(owner) =
            super::super::async_worker::register_worker_if_absent(
                key.clone(),
                sender,
                pending.clone(),
                true,
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
            })?
        {
            spawn_async_delivery_worker(
                key.clone(),
                owner,
                receiver,
                pending,
                WorkerTransportSource::Pty(bootstrap),
            );
        }
    }
    // The worker unregisters on construction failure, taking its entry
    // with it — so the recorded cause lives in the construction-error
    // table, not the entry. Consuming (not peeking) is what keeps a stale
    // cause from a previous generation from describing this one; a
    // successful build clears the table, and each arm below returns, so
    // the take runs at most once per bring-up.
    //
    // Prime wait: registration alone must not report ready. The transport
    // publishes `Available` once its child is spawned and its terminal
    // established — the same acceptance set `list` reports, so a session
    // this poll counts ready is one `list` also calls ready.
    // (`acp_readiness_is_ready` is reused for the predicate; the name is
    // ACP-specific, the `Available | Busy` acceptance is
    // transport-neutral.)
    let deadline =
        Instant::now() + Duration::from_millis(PTY_STARTUP_PRIME_TIMEOUT_MS);
    loop {
        if shutdown_requested() {
            return Err(startup_interrupted_error(target_member.id.as_str()));
        }
        // Vanished after this pass registered (or found) it: the worker's
        // first generation build failed and it unregistered. That is the
        // prompt signal, not the deadline — a failed construction must not
        // wait out the bound.
        if !super::super::async_worker::worker_exists(&key).map_err(
            |error| {
                (
                    "internal_unexpected_failure".to_string(),
                    "failed to query Pty worker registry".to_string(),
                    Some(json!({
                        "target_session": target_member.id,
                        "cause": error.message,
                    })),
                )
            },
        )? {
            return Err(startup_failure_error(
                take_construction_error(&key),
                "runtime_pty_startup_failed",
                "Pty worker unregistered during startup",
                json!({
                    "target_session": target_member.id,
                }),
            ));
        }
        let readiness = get_worker_readiness(
            namespace,
            runtime_directory,
            target_member.id.as_str(),
        );
        if super::super::async_worker::acp_readiness_is_ready(readiness) {
            return Ok(());
        }
        match readiness {
            Some(WorkerReadinessState::Unavailable) => {
                return Err(startup_failure_error(
                    take_construction_error(&key),
                    "runtime_pty_worker_unavailable",
                    "Pty worker is unavailable after startup",
                    json!({
                        "target_session": target_member.id,
                    }),
                ));
            },
            // Still settling. `Available`/`Busy` returned above, so this arm
            // is reached only for `Initializing`, `Recovering`, and — already
            // excluded by the vanished check — an unregistered worker.
            _ => {
                if Instant::now() >= deadline {
                    return Err(startup_failure_error(
                        take_construction_error(&key),
                        "runtime_startup_failed",
                        "Pty worker did not become ready during startup",
                        json!({
                            "target_session": target_member.id,
                            "timeout_ms": PTY_STARTUP_PRIME_TIMEOUT_MS,
                        }),
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            },
        }
    }
}

/// Reports a startup abandoned because the relay is shutting down.
fn startup_interrupted_error(
    target_session: &str,
) -> (String, String, Option<serde_json::Value>) {
    (
        "runtime_startup_interrupted".to_string(),
        "relay shutdown requested during ACP worker startup".to_string(),
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
    task: AsyncDeliveryTask,
) -> Result<(), RelayError> {
    enqueue_delivery_task(task)
}

fn enqueue_delivery_task(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let bounded_acp_queue =
        super::super::async_worker::task_uses_acp_transport(&task);
    let key = super::super::async_worker::build_worker_key(
        task.bundle.bundle_name.as_str(),
        task.runtime_directory.as_path(),
        task.target_session.as_str(),
    );
    if bounded_acp_queue && !super::super::async_worker::worker_exists(&key)? {
        return Err(super::super::super::relay_error(
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": task.target_session,
            })),
        ));
    }
    match super::super::async_worker::try_existing_worker(&key, task)? {
        WorkerDispatch::Accepted => Ok(()),
        WorkerDispatch::Closing(task) => {
            // The target's worker is draining for relay shutdown. Record the drop
            // on the observability floor and stop; spawning here would resurrect a
            // worker mid-shutdown and clobber the closing registry entry the
            // shutdown barrier still counts (an accept-after-drain regression).
            super::super::async_worker::complete_task_on_shutdown(&task);
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
            let transport_context = WorkerTransportContext::resolve(&task)?;
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
                key.clone(),
                sender.clone(),
                pending.clone(),
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
                None => dispatch_to_installed_owner(&key, task),
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
    key: &super::super::async_worker::AsyncWorkerKey,
    task: AsyncDeliveryTask,
) -> Result<(), RelayError> {
    match super::super::async_worker::try_existing_worker(key, task)? {
        WorkerDispatch::Accepted => Ok(()),
        WorkerDispatch::Closing(task) => {
            super::super::async_worker::complete_task_on_shutdown(&task);
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
mod tests {
    // The enqueue path's `Closing` arm is crate-private and unreachable through any
    // public interface without a live relay shutdown. This inline test registers a
    // closing target worker and drives `enqueue_delivery_task` against it, observing
    // that the raced task is dropped as a `DroppedOnShutdown` receipt to the
    // sender's worker while the closing entry is retained and fed nothing — no
    // replacement worker is spawned over it. One `#[test]`, no widened visibility.
    use super::super::super::async_worker::{
        build_worker_key, close_worker, register_worker, unregister_worker,
        worker_exists,
    };
    use super::*;
    use crate::configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration,
    };
    use crate::relay::{
        DeliveryPayloadMode, SCHEMA_VERSION, SenderReturnRoute,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn tmux_member(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Tmux(
                crate::configuration::TmuxTargetConfiguration {
                    start_command: format!("run-{id}"),
                    prompt_readiness: None,
                },
            ),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    #[cfg(feature = "pty")]
    fn pty_member_with_unparseable_command(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Pty(
                crate::configuration::PtyTargetConfiguration {
                    // Unterminated quote: tokenization fails before any
                    // spawn, so construction failure is deterministic with
                    // no child process and no timing dependence.
                    initial_command: "sh -c \"echo hi".to_string(),
                    resume_command: String::new(),
                    prompt_readiness: None,
                    cols: 120,
                    rows: 40,
                    term_protocol: crate::configuration::TermProtocol::default(
                    ),
                },
            ),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    /// A Send racing relay shutdown whose target worker is already draining
    /// (`closing`) is dropped as `DroppedOnShutdown`, not spawned into a replacement
    /// worker that would clobber the closing registry entry the shutdown barrier
    /// still counts. The dropped task surfaces to the sender as a receipt; the
    /// closing worker is fed nothing and stays registered.
    #[test]
    fn enqueue_drops_task_targeting_a_closing_worker() {
        let target_namespace = "enqueue-target-ns";
        let target_runtime = "/enqueue-target-rt";
        let target_session = "enqueue-target-sess";
        let sender_namespace = "enqueue-sender-ns";
        let sender_runtime = "/enqueue-sender-rt";
        let sender_member_id = "enqueue-sender-mem";

        // A registered, closing target worker; enqueue must not feed it.
        let target_key = build_worker_key(
            target_namespace,
            Path::new(target_runtime),
            target_session,
        );
        let (target_tx, mut target_rx) =
            tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let target_owner = register_worker(
            target_key.clone(),
            target_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        close_worker(&target_key, target_owner);

        // A live sender worker: the DroppedOnShutdown receipt should land here.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            sender_key.clone(),
            sender_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );

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
            sender: tmux_member(sender_member_id),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: target_session.to_string(),
            message: "body".to_string(),
            message_id: "orig-message-id".to_string(),
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

        enqueue_delivery_task(task)
            .expect("enqueue returns Ok when the target worker is closing");

        // The closing worker is retained and was fed nothing: no replacement worker
        // spawned or registered over the closing entry.
        assert!(
            worker_exists(&target_key).expect("registry lock"),
            "the closing worker entry is retained"
        );
        assert!(
            target_rx.try_recv().is_err(),
            "the closing worker must not be fed the raced task"
        );

        // The raced task resolved DroppedOnShutdown, surfaced to the sender as a
        // receipt through the sender's own worker.
        let receipt = sender_rx
            .try_recv()
            .expect("the dropped task yields a DroppedOnShutdown receipt to the sender");
        assert!(receipt.is_receipt);
        assert!(
            receipt.message.contains("dropped on relay shutdown"),
            "receipt reports the dropped-on-shutdown outcome: {}",
            receipt.message
        );

        unregister_worker(&target_key, target_owner);
        unregister_worker(&sender_key, owner);
    }

    /// A Pty member whose child cannot be constructed records a startup
    /// failure instead of ready.
    ///
    /// The load-bearing verification for eager construction: registration
    /// alone must not report ready. The unparseable command fails
    /// deterministically before any spawn, so the worker's first generation
    /// build fails, the worker unregisters, and the prime wait must observe
    /// the vanished worker (or its `Unavailable` transition) and report —
    /// promptly, not at the prime-timeout bound.
    #[cfg(feature = "pty")]
    #[test]
    fn pty_bring_up_with_unconstructible_child_records_startup_failure() {
        let namespace = "pty-startup-failure-ns";
        let runtime = tempfile::TempDir::new().expect("temporary directory");
        let member = pty_member_with_unparseable_command("pty-bad-child");
        let before = std::time::Instant::now();
        let result = initialize_pty_target_for_startup(
            namespace,
            runtime.path(),
            &member,
        );
        let elapsed = before.elapsed();
        let (code, reason, _) = result.expect_err(
            "an unconstructible Pty child must fail bring-up, not record ready",
        );
        assert!(
            elapsed < std::time::Duration::from_millis(2_000),
            "the failure must surface from the vanished worker, not the prime-timeout bound: {elapsed:?}",
        );
        assert_eq!(
            code, "pty_command_parse_failed",
            "the recorded transport cause survives the worker's unregister: {reason}",
        );
        assert!(
            !worker_exists(&build_worker_key(
                namespace,
                runtime.path(),
                "pty-bad-child",
            ))
            .expect("registry lock"),
            "a failed construction leaves no worker registered",
        );
    }

    #[cfg(feature = "pty")]
    fn pty_member_with_live_command(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Pty(
                crate::configuration::PtyTargetConfiguration {
                    // A child that outlives the test: prime-wait
                    // termination is about the worker's `Available`
                    // publish, and an instantly-exiting child would
                    // race the departure publish against it.
                    initial_command: "sleep 30".to_string(),
                    resume_command: String::new(),
                    prompt_readiness: None,
                    cols: 120,
                    rows: 40,
                    term_protocol: crate::configuration::TermProtocol::default(
                    ),
                },
            ),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    /// Bring-up elects the Pty worker eagerly, and a repeated pass adopts
    /// rather than replaces it.
    ///
    /// The first call registers the worker and the prime wait observes its
    /// `Available` transition; the second call — what reconcile/`up` runs —
    /// finds the entry and leaves it alone. Either call racing a first
    /// delivery resolves through the same registry election, which is what
    /// keeps one worker (and therefore one child) per target.
    #[cfg(feature = "pty")]
    #[test]
    fn pty_bring_up_elects_worker_and_repeat_pass_adopts_it() {
        use super::super::super::async_worker::{
            stop_workers_for_bundle, wait_for_bundle_workers_stopped,
        };

        let namespace = "pty-startup-ok-ns";
        let runtime = tempfile::TempDir::new().expect("temporary directory");
        let member = pty_member_with_live_command("pty-live-child");
        initialize_pty_target_for_startup(namespace, runtime.path(), &member)
            .expect("a constructible Pty child records ready at bring-up");
        let key =
            build_worker_key(namespace, runtime.path(), "pty-live-child");
        assert!(
            worker_exists(&key).expect("registry lock"),
            "bring-up leaves the worker registered for first delivery to adopt",
        );
        assert!(
            super::super::super::async_worker::acp_readiness_is_ready(
                super::super::super::async_worker::get_worker_readiness(
                    namespace,
                    runtime.path(),
                    "pty-live-child",
                ),
            ),
            "the primed worker is serving, not still initializing",
        );
        // The repeat pass is reconcile/`up`: same answer, no replacement.
        initialize_pty_target_for_startup(namespace, runtime.path(), &member)
            .expect("a repeat pass over a live worker stays ready");
        assert!(
            worker_exists(&key).expect("registry lock"),
            "the repeat pass adopts the worker rather than replacing it",
        );
        stop_workers_for_bundle(namespace, runtime.path());
        wait_for_bundle_workers_stopped(
            namespace,
            runtime.path(),
            std::time::Duration::from_secs(10),
        );
        assert!(
            !worker_exists(&key).expect("registry lock"),
            "the test's worker stops cleanly with its child",
        );
    }
}
