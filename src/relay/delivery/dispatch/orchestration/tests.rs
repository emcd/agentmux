//! Prime-wait and election tests for the delivery orchestration.
//!
//! Split from the orchestration module under the line-count gate:
//! each test keeps its own one-test module with shared fixtures,
//! as in the parent file. The enqueue path's arms are crate-private,
//! so these stay inline rather than moving to `tests/`.
#[cfg(test)]
mod enqueue_fixtures {
    // Shared member fixture for the one-test modules below: each test
    // lives in its own inline module (one `#[test]` per block) because
    // the enqueue path's arms are crate-private, and widening visibility
    // to reach them from `tests/` would publish delivery internals.
    use crate::configuration::{BundleMember, TargetConfiguration};

    pub(super) fn tmux_member(id: &str) -> BundleMember {
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
}

/// The enqueue path's `Closing` arm is crate-private and unreachable through any
/// public interface without a live relay shutdown. This inline test registers a
/// closing target worker and drives `enqueue_delivery_task` against it, observing
/// that the raced task is dropped as a `DroppedOnShutdown` receipt to the
/// sender's worker while the closing entry is retained and fed nothing — no
/// replacement worker is spawned over it. One `#[test]`, no widened visibility.
#[cfg(test)]
mod closing_drop_tests {
    use super::super::super::super::async_worker::{
        build_worker_key, close_worker, register_worker, unregister_worker,
        worker_exists,
    };
    use super::super::*;
    use super::enqueue_fixtures::*;
    use crate::configuration::BundleConfiguration;
    use crate::relay::{
        DeliveryPayloadMode, SCHEMA_VERSION, SenderReturnRoute,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// A Send racing relay shutdown whose target worker is already draining
    /// (`closing`) is dropped as `DroppedOnShutdown`, not spawned into a replacement
    /// worker that would clobber the closing registry entry the shutdown barrier
    /// still counts. The dropped task surfaces to the sender as a receipt; the
    /// closing worker is fed nothing and stays registered.
    #[test]
    fn enqueue_drops_task_targeting_a_closing_worker() {
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
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
            delivery.async_registry(),
            target_key.clone(),
            target_tx,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        close_worker(delivery.async_registry(), &target_key, target_owner);

        // A live sender worker: the DroppedOnShutdown receipt should land here.
        let sender_key = build_worker_key(
            sender_namespace,
            Path::new(sender_runtime),
            sender_member_id,
        );
        let (sender_tx, mut sender_rx) =
            tokio_mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            delivery.async_registry(),
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

        enqueue_delivery_task(&delivery, task)
            .expect("enqueue returns Ok when the target worker is closing");

        // The closing worker is retained and was fed nothing: no replacement worker
        // spawned or registered over the closing entry.
        assert!(
            worker_exists(delivery.async_registry(), &target_key)
                .expect("registry lock"),
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

        unregister_worker(
            &delivery,
            delivery.async_registry(),
            &target_key,
            target_owner,
        );
        unregister_worker(
            &delivery,
            delivery.async_registry(),
            &sender_key,
            owner,
        );
    }
}

/// Lazy-election queue policy is crate-private (the Missing/Dropped arm is
/// unreachable without driving the real enqueue path), so this inline test
/// is the seam: one `#[test]`, no widened visibility.
#[cfg(test)]
mod lazy_queue_tests {
    use super::super::*;
    use super::enqueue_fixtures::*;
    use crate::configuration::BundleConfiguration;
    use crate::relay::{DeliveryPayloadMode, SCHEMA_VERSION};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Lazy-election queue policy: the delivery-time Missing/Dropped arm
    /// elects workers for every non-ACP transport, so its registration
    /// stays unbounded. Seventy sends against a worker that never consumes
    /// (entered but unpolled runtime: spawns succeed, nothing runs, no
    /// slot is ever released) must all succeed; with the ACP cap
    /// reimposed here, sends past the 64-slot bound fail
    /// `runtime_acp_queue_full`.
    #[test]
    fn lazy_election_keeps_non_acp_workers_unbounded() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");
        let _entered = runtime.enter();
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        let target_namespace = "lazy-queue-target-ns";
        let target_runtime = "/lazy-queue-target-rt";
        let target_session = "lazy-queue-target-sess";
        let sender_member_id = "lazy-queue-sender-mem";

        let send = |index: usize| AsyncDeliveryTask {
            // Test fixture: constructed directly, never admitted.
            admitted: false,
            bundle: BundleConfiguration {
                schema_version: SCHEMA_VERSION.to_string(),
                bundle_name: target_namespace.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: vec![tmux_member(target_session)],
            },
            sender_namespace: target_namespace.to_string(),
            sender: tmux_member(sender_member_id),
            authenticated_identity: None,
            on_behalf_of: None,
            all_target_sessions: Vec::new(),
            target_session: target_session.to_string(),
            message: format!("body-{index}"),
            message_id: format!("lazy-queue-{index}"),
            runtime_directory: PathBuf::from(target_runtime),
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: None,
        };

        let mut refused = Vec::new();
        for index in 0..70 {
            if let Err(error) = enqueue_delivery_task(&delivery, send(index)) {
                refused.push((index, error.code));
            }
        }
        assert!(
            refused.is_empty(),
            "every send to a lazily elected non-ACP worker must be accepted, refused: {refused:?}"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod pty_startup_fixtures {
    // Shared member fixture for the Pty bring-up one-test modules below:
    // each test lives in its own inline module (one `#[test]` per block)
    // because the initializer is crate-private, and widening visibility to
    // reach it from `tests/` would publish delivery internals.
    use crate::configuration::{BundleMember, TargetConfiguration};

    pub(super) fn pty_member(id: &str) -> BundleMember {
        BundleMember {
            id: id.to_string(),
            name: Some(id.to_string()),
            working_directory: None,
            target: TargetConfiguration::Pty(
                crate::configuration::PtyTargetConfiguration {
                    initial_command: String::new(),
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
}
#[cfg(all(test, feature = "pty"))]
mod startup_refused_when_signalled_tests {
    use super::super::super::super::async_worker::{
        adopt_registered_attempt, build_worker_key,
    };
    use super::super::*;
    use super::pty_startup_fixtures::*;
    use std::path::Path;
    use std::sync::Arc;

    /// A signalled instance gate refuses Pty bring-up before any election:
    /// no entry is installed and no child is spawned, even though the
    /// process-wide signal is quiet.
    #[test]
    fn startup_refused_when_signalled_before_election() {
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        delivery.shutdown_gate().signal_shutdown();
        let member = pty_member("refused-sess");
        let error = initialize_pty_target_for_startup(
            &delivery,
            "refused-ns",
            Path::new("/refused-rt"),
            &member,
        )
        .expect_err("a signalled gate refuses Pty startup");
        assert_eq!(error.0, "runtime_startup_interrupted");
        let registry = delivery.async_registry();
        let key = build_worker_key(
            "refused-ns",
            Path::new("/refused-rt"),
            "refused-sess",
        );
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "no worker is elected after a refused startup"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod staged_gate_signal_tests {
    use super::super::super::super::async_worker::{
        adopt_registered_attempt, build_worker_key, register_worker,
        unregister_worker,
    };
    use super::super::*;
    use super::pty_startup_fixtures::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;

    /// A gate signalled mid-wait interrupts the prime wait with the
    /// interruption error rather than reporting the (never-coming) ready
    /// state. The entry pre-exists so no worker spawns and no child runs.
    /// The barrier stages the signal strictly after the initializer
    /// starts: the pre-check runs within microseconds of the rendezvous
    /// while the signal lands a full staged interval later, so the test
    /// deterministically exercises the in-wait gate check rather than the
    /// pre-election refusal (which has its own test).
    #[test]
    fn staged_gate_signal_interrupts_prime_wait() {
        use std::sync::Barrier;
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        let registry = delivery.async_registry();
        let key = build_worker_key(
            "staged-gate-ns",
            Path::new("/staged-gate-rt"),
            "staged-gate-sess",
        );
        let (sender, _) = unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        let barrier = Arc::new(Barrier::new(2));
        let gate = Arc::clone(&delivery);
        let gate_barrier = Arc::clone(&barrier);
        let signaller = std::thread::spawn(move || {
            gate_barrier.wait();
            std::thread::sleep(std::time::Duration::from_millis(50));
            gate.shutdown_gate().signal_shutdown();
        });
        barrier.wait();
        let member = pty_member("staged-gate-sess");
        let error = initialize_pty_target_for_startup(
            &delivery,
            "staged-gate-ns",
            Path::new("/staged-gate-rt"),
            &member,
        )
        .expect_err("a mid-wait gate signal interrupts startup");
        assert_eq!(error.0, "runtime_startup_interrupted");
        signaller.join().expect("signaller joins");
        unregister_worker(&delivery, registry, &key, owner);
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "cleanup removes the entry"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod unavailable_without_cause_tests {
    use super::super::super::super::async_worker::{
        adopt_registered_attempt, build_worker_key, register_worker,
        set_worker_readiness, unregister_worker,
    };
    use super::super::*;
    use super::pty_startup_fixtures::*;
    use crate::transports::WorkerReadinessState;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;

    /// A registered Unavailable with no construction cause fails promptly
    /// with the Unavailable phase detail instead of burning the prime bound
    /// on a generic timeout. The entry pre-exists so no worker spawns.
    #[test]
    fn unavailable_without_cause_fails_promptly() {
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        let registry = delivery.async_registry();
        let key = build_worker_key(
            "unavailable-ns",
            Path::new("/unavailable-rt"),
            "unavailable-sess",
        );
        let (sender, _) = unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        set_worker_readiness(
            registry,
            "unavailable-ns",
            Path::new("/unavailable-rt"),
            "unavailable-sess",
            WorkerReadinessState::Unavailable,
        );
        let member = pty_member("unavailable-sess");
        let error = initialize_pty_target_for_startup(
            &delivery,
            "unavailable-ns",
            Path::new("/unavailable-rt"),
            &member,
        )
        .expect_err("Unavailable without a cause still fails startup");
        assert_eq!(error.0, "runtime_pty_unavailable");
        unregister_worker(&delivery, registry, &key, owner);
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "cleanup removes the entry"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod unavailable_before_cause_tests {
    use super::super::super::super::async_worker::{
        adopt_registered_attempt, build_worker_key, record_construction_error,
        register_worker, set_worker_readiness, unregister_worker,
    };
    use super::super::*;
    use super::pty_startup_fixtures::*;
    use crate::transports::{WorkerFailureReason, WorkerReadinessState};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;

    /// A recorded cause wins over an observed Unavailable: construction
    /// records before it publishes the terminal registry state, so an
    /// Unavailable observation with a cause present always prefers the
    /// cause. No threads, no timing: the record strictly precedes the
    /// observation.
    #[test]
    fn unavailable_before_cause_reports_cause() {
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        let registry = delivery.async_registry();
        let key = build_worker_key(
            "unavailable-cause-ns",
            Path::new("/unavailable-cause-rt"),
            "unavailable-cause-sess",
        );
        let (sender, _) = unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        assert!(
            record_construction_error(
                registry,
                &key,
                owner,
                WorkerFailureReason {
                    code: "pty_spawn_failed".to_string(),
                    reason: "spawn: no such program".to_string(),
                },
            ),
            "the record lands on the waited attempt"
        );
        set_worker_readiness(
            registry,
            "unavailable-cause-ns",
            Path::new("/unavailable-cause-rt"),
            "unavailable-cause-sess",
            WorkerReadinessState::Unavailable,
        );
        let member = pty_member("unavailable-cause-sess");
        let error = initialize_pty_target_for_startup(
            &delivery,
            "unavailable-cause-ns",
            Path::new("/unavailable-cause-rt"),
            &member,
        )
        .expect_err("Unavailable with a cause reports the cause");
        assert_eq!(error.0, "pty_spawn_failed");
        unregister_worker(&delivery, registry, &key, owner);
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "cleanup removes the entry"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod stopping_entry_not_serving_tests {
    use super::super::super::super::async_worker::{
        build_worker_key, register_worker, set_worker_readiness,
        stop_workers_for_bundle, unregister_worker,
    };
    use super::super::*;
    use super::pty_startup_fixtures::*;
    use crate::transports::WorkerReadinessState;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;

    /// A stopping entry that reads Available is never reported serving:
    /// stopping is published before readiness here, so no poll can ever
    /// observe a serving entry that is not already stopping — under the
    /// old gate the first poll after the publish returns Ok, under the new
    /// gate the drain's unregister ends the wait instead. The entry
    /// pre-exists so no worker spawns and no gate is signalled.
    #[test]
    fn stopping_entry_never_reports_serving() {
        let delivery = Arc::new(crate::relay::delivery::DeliveryContext::new(
            Default::default(),
        ));
        let registry = delivery.async_registry();
        let key = build_worker_key(
            "stopping-ns",
            Path::new("/stopping-rt"),
            "stopping-sess",
        );
        let (sender, _) = unbounded_channel::<AsyncDeliveryTask>();
        let owner = register_worker(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        // The drain thread sleeps first so the pre-election guard passes
        // deterministically; it then stops and publishes while the prime
        // wait is polling, so the serving gate (not the pre-check) is what
        // refuses the entry. Stop precedes publish within the thread, so
        // no poll can ever see a serving entry that is not already
        // stopping — any Ok would be the old gate. The drain's unregister
        // ends the wait promptly instead of burning the prime bound.
        let drain = Arc::clone(&delivery);
        let drain_key = key.clone();
        let drain_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            stop_workers_for_bundle(
                drain.async_registry(),
                "stopping-ns",
                Path::new("/stopping-rt"),
            );
            set_worker_readiness(
                drain.async_registry(),
                "stopping-ns",
                Path::new("/stopping-rt"),
                "stopping-sess",
                WorkerReadinessState::Available,
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            unregister_worker(
                &drain,
                drain.async_registry(),
                &drain_key,
                owner,
            );
        });
        let member = pty_member("stopping-sess");
        let error = initialize_pty_target_for_startup(
            &delivery,
            "stopping-ns",
            Path::new("/stopping-rt"),
            &member,
        )
        .expect_err("a stopping entry is never reported serving");
        assert_eq!(error.0, "runtime_pty_startup_failed");
        drain_handle.join().expect("drain thread joins");
    }
}
