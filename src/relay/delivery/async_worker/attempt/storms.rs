//! Replacement-pressure storms for the attempt-handle contract.
//!
//! Split from the attempt module under the line-count gate: each
//! test keeps its own one-test module, as in the parent file.
#[cfg(all(test, feature = "pty"))]
mod adopt_storm_tests {
    use super::super::super::reap::unregister_worker;
    use super::super::construction_completion_fixtures::*;
    use super::super::*;
    use std::sync::atomic::AtomicUsize;

    /// Adoption storm: a registrar installs and records a fresh attempt
    /// every round while a replacer churns the entry and adopters assert
    /// every logged adoption.
    ///
    /// Each round installs a new owner, records a UNIQUE cause for it, and
    /// logs the owner only if the record landed. An adopter that finds its
    /// owner logged knows the record happens-before (mutex order), so the
    /// handle must read that exact cause. A split owner/handle acquisition
    /// lets the round's replacement land in between: the waiter names the
    /// old owner but holds the new attempt's handle, whose cause differs.
    /// The per-round barrier guarantees every round starts with a fresh
    /// logged attempt present, so liveness does not depend on scheduling
    /// luck; the replacer churns every round so the race is structural.
    /// Mismatches are collected, not asserted: an in-scope panic would
    /// strand the other threads at the barrier and turn a fast failure
    /// into a timeout.
    #[test]
    fn adopters_under_replacement_storm_observe_their_attempts_result() {
        use std::collections::HashMap;
        use std::sync::{Barrier, Mutex};
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let target = key("construction-adopt-storm");
        let logged: Mutex<HashMap<WorkerOwner, String>> =
            Mutex::new(HashMap::new());
        let asserted = AtomicUsize::new(0);
        let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let barrier = Barrier::new(4);
        // Outcome counters for the failure message: a liveness shortfall
        // reads very differently with none/unlogged/mismatch broken out.
        let saw_none = AtomicUsize::new(0);
        let saw_unlogged = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            // Registrar: fresh recorded attempt per round, logged only if
            // recorded. Gating the log on the record is what keeps an
            // unrecorded owner out of the log.
            let registrar = scope.spawn(|| {
                for round in 0..100 {
                    let owner = install(registry, &target);
                    let code = format!("pty_attempt_{round}");
                    if record_construction_error(
                        registry,
                        &target,
                        owner,
                        cause(&code),
                    ) {
                        logged.lock().expect("log mutex").insert(owner, code);
                    }
                    barrier.wait();
                    barrier.wait();
                }
            });
            // Replacer: install afresh every iteration, recording and logging
            // each installation, so every resident attempt stays assertable
            // for the adopters. Same replacing install as the registrar —
            // one discipline across both writers — and no unregister gaps:
            // an absent entry only spends waiter iterations on nothing.
            // Each installation records a UNIQUE cause (round and index),
            // so any split owner/handle pairing names a code its handle
            // never recorded.
            let churn = scope.spawn(|| {
                for round in 0..100 {
                    barrier.wait();
                    for i in 0..30 {
                        let owner = install(registry, &target);
                        let code = format!("pty_churn_{round}_{i}");
                        if record_construction_error(
                            registry,
                            &target,
                            owner,
                            cause(&code),
                        ) {
                            logged
                                .lock()
                                .expect("log mutex")
                                .insert(owner, code);
                        }
                    }
                    barrier.wait();
                }
            });
            // Adopters: every logged adoption must read its own exact
            // cause; unlogged adoptions assert nothing. A logged owner
            // whose handle reads otherwise — or reads nothing — is the
            // split-acquisition defect.
            let mut waiters = Vec::new();
            for _ in 0..2 {
                waiters.push(scope.spawn(|| {
                    for _ in 0..100 {
                        barrier.wait();
                        for _ in 0..30 {
                            if let Some((adopted, handle)) =
                                adopt_registered_attempt(registry, &target)
                            {
                                let guard =
                                    logged.lock().expect("log mutex");
                                if let Some(expected) =
                                    guard.get(&adopted)
                                {
                                    match handle.read() {
                                        Some(recorded)
                                            if recorded.code == *expected =>
                                        {
                                            asserted.fetch_add(
                                                1,
                                                std::sync::atomic::Ordering::Relaxed,
                                            );
                                        },
                                        observed => {
                                            failures
                                                .lock()
                                                .expect("failures mutex")
                                                .push(format!(
                                                    "logged attempt {adopted:?} observed {observed:?}, expected {expected}"
                                                ));
                                        },
                                    }
                                } else {
                                    saw_unlogged.fetch_add(
                                        1,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                }
                            } else {
                                saw_none.fetch_add(
                                    1,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                        }
                        barrier.wait();
                    }
                }));
            }
            registrar.join().expect("registrar joins");
            churn.join().expect("churn joins");
            for waiter in waiters {
                waiter.join().expect("waiter joins");
            }
        });
        assert!(
            asserted.load(std::sync::atomic::Ordering::Relaxed) > 100,
            "the storm must have asserted logged adoptions: asserted={} none={} unlogged={}",
            asserted.load(std::sync::atomic::Ordering::Relaxed),
            saw_none.load(std::sync::atomic::Ordering::Relaxed),
            saw_unlogged.load(std::sync::atomic::Ordering::Relaxed),
        );
        assert!(
            failures.lock().expect("failures mutex").is_empty(),
            "no adoption may observe another attempt's cause: {}",
            failures.lock().expect("failures mutex").join("; ")
        );
        if let Some((current, _)) = adopt_registered_attempt(registry, &target)
        {
            unregister_worker(&delivery, registry, &target, current);
        }
    }
}
#[cfg(all(test, feature = "pty"))]
mod snapshot_storm_tests {
    use super::super::super::reap::unregister_worker;
    use super::super::super::register_worker_with_readiness;
    use super::super::construction_completion_fixtures::*;
    use super::super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// Snapshot storm: a registrar installs a fresh serving attempt every
    /// round while a replacer churns and an observer asserts
    /// owner-attribution through the production snapshot primitive.
    ///
    /// Every installation is born serving: the entry names its readiness in
    /// the same insert, so no publish gap follows the install and every
    /// snapshot of a current owner reads the state that owner published.
    /// Installations cycle through three readiness states in install order,
    /// and each owner is logged with the state it published. A snapshot
    /// naming a logged owner must therefore read exactly that owner's
    /// state: a split owner/readiness acquisition pairs the old owner with
    /// the replacement's state whenever the replacement lands in the gap,
    /// and the per-round replacement makes that gap land often. Mismatches
    /// are collected, not asserted, for the same reason as the adoption
    /// storm. Both writers use the same replacing install discipline, so
    /// the storm tests replacement rather than one thread silently
    /// refusing the other's entry.
    #[test]
    fn snapshots_under_replacement_storm_attribute_serving_to_its_owner() {
        use std::collections::HashMap;
        use std::sync::{Barrier, Mutex};
        const STATES: [WorkerReadinessState; 3] = [
            WorkerReadinessState::Available,
            WorkerReadinessState::Busy,
            WorkerReadinessState::Recovering,
        ];
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let target = key("construction-snapshot-storm");
        let logged: Mutex<HashMap<WorkerOwner, WorkerReadinessState>> =
            Mutex::new(HashMap::new());
        let asserted = AtomicUsize::new(0);
        let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let barrier = Barrier::new(3);
        // Install order across both writers: consecutive installs publish
        // different states, so a torn pair always mismatches its owner.
        let installs = AtomicUsize::new(0);
        let install_published = |state: WorkerReadinessState| {
            let (sender, _) =
                tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
            register_worker_with_readiness(
                registry,
                target.clone(),
                sender,
                Arc::new(AtomicUsize::new(0)),
                false,
                state,
            )
        };
        std::thread::scope(|scope| {
            let registrar = scope.spawn(|| {
                for _ in 0..100 {
                    let state = STATES[installs
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        % STATES.len()];
                    let owner = install_published(state);
                    logged.lock().expect("log mutex").insert(owner, state);
                    barrier.wait();
                    barrier.wait();
                }
            });
            let churn = scope.spawn(|| {
                for _ in 0..100 {
                    barrier.wait();
                    for _ in 0..30 {
                        let state = STATES[installs.fetch_add(
                            1,
                            std::sync::atomic::Ordering::Relaxed,
                        ) % STATES.len()];
                        let owner = install_published(state);
                        logged.lock().expect("log mutex").insert(owner, state);
                    }
                    barrier.wait();
                }
            });
            let observer = scope.spawn(|| {
                for _ in 0..100 {
                    barrier.wait();
                    for _ in 0..30 {
                        if let Some((snapshot_owner, readiness, _stopping)) =
                            worker_attempt_snapshot(registry, &target)
                            && let Some(expected) = logged
                                .lock()
                                .expect("log mutex")
                                .get(&snapshot_owner)
                        {
                            if Some(*expected) == readiness {
                                asserted.fetch_add(
                                    1,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            } else {
                                failures.lock().expect("failures mutex").push(
                                    format!(
                                        "logged owner {snapshot_owner:?} published {expected:?} but observed {readiness:?}"
                                    ),
                                );
                            }
                        }
                    }
                    barrier.wait();
                }
            });
            registrar.join().expect("registrar joins");
            churn.join().expect("churn joins");
            observer.join().expect("observer joins");
        });
        assert!(
            asserted.load(std::sync::atomic::Ordering::Relaxed) > 100,
            "the storm must have asserted logged snapshots: asserted={}",
            asserted.load(std::sync::atomic::Ordering::Relaxed),
        );
        assert!(
            failures.lock().expect("failures mutex").is_empty(),
            "no snapshot may attribute another attempt's readiness: {}",
            failures.lock().expect("failures mutex").join("; ")
        );
        if let Some((current, _)) = adopt_registered_attempt(registry, &target)
        {
            unregister_worker(&delivery, registry, &target, current);
        }
    }
}
#[cfg(all(test, feature = "pty"))]
mod snapshot_stopping_tests {
    use super::super::super::reap::unregister_worker;
    use super::super::super::registry::{
        acp_readiness_is_ready, set_worker_readiness, stop_workers_for_bundle,
    };
    use super::super::construction_completion_fixtures::*;
    use super::super::*;

    /// The snapshot carries the entry's stopping state in the same
    /// acquisition as owner and readiness, so the prime wait's serving
    /// decision rechecks lifecycle eligibility on every poll instead of
    /// relying on the pre-election guard alone.
    #[test]
    fn snapshot_reports_stopping_alongside_owner_and_readiness() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-snapshot-stopping");
        let owner = install(registry, &key);
        set_worker_readiness(
            registry,
            key.namespace.as_str(),
            key.runtime_directory.as_path(),
            key.target_session.as_str(),
            WorkerReadinessState::Available,
        );
        let (snapshot_owner, readiness, stopping) =
            worker_attempt_snapshot(registry, &key).expect("registered");
        assert_eq!(snapshot_owner, owner, "snapshot names the owner");
        assert!(
            acp_readiness_is_ready(readiness),
            "serving is observable before the stop"
        );
        assert!(!stopping, "nothing stopped yet");
        stop_workers_for_bundle(
            registry,
            key.namespace.as_str(),
            key.runtime_directory.as_path(),
        );
        let (stopped_owner, stopped_readiness, stopped) =
            worker_attempt_snapshot(registry, &key).expect("still registered");
        assert_eq!(stopped_owner, owner, "the stop names no successor");
        assert!(
            acp_readiness_is_ready(stopped_readiness),
            "readiness is unchanged by the stop"
        );
        assert!(
            stopped,
            "an entry stopping after a valid ready observation reports it"
        );
        unregister_worker(&delivery, registry, &key, owner);
    }
}
#[cfg(all(test, feature = "pty"))]
mod owner_checked_readiness_tests {
    use super::super::super::reap::unregister_worker;
    use super::super::super::registry::{
        acp_readiness_is_ready, set_worker_readiness_if_owner,
    };
    use super::super::construction_completion_fixtures::*;
    use super::super::*;

    fn publish(
        registry: &AsyncDeliveryRegistry,
        owner: WorkerOwner,
        key: &AsyncWorkerKey,
        state: WorkerReadinessState,
    ) {
        set_worker_readiness_if_owner(
            registry,
            owner,
            key.namespace.as_str(),
            key.runtime_directory.as_path(),
            key.target_session.as_str(),
            state,
        );
    }

    /// A late publisher cannot describe its successor: publishing with a
    /// superseded owner leaves the entry untouched, while the current
    /// owner still publishes. Owner and readiness move together, so the
    /// prime wait never treats another attempt's state as its waited
    /// result.
    #[test]
    fn stale_owner_publish_leaves_successor_untouched() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-owner-checked");
        let first = install(registry, &key);
        publish(registry, first, &key, WorkerReadinessState::Available);
        let (_, readiness, _) =
            worker_attempt_snapshot(registry, &key).expect("registered");
        assert!(
            acp_readiness_is_ready(readiness),
            "the current owner publishes"
        );
        let second = install(registry, &key);
        assert_ne!(second, first, "the test needs a successor");
        publish(registry, first, &key, WorkerReadinessState::Busy);
        let (owner, readiness, _) =
            worker_attempt_snapshot(registry, &key).expect("replaced");
        assert_eq!(owner, second, "the entry names the successor");
        assert!(
            readiness.is_none(),
            "the stale publish changed nothing: no readiness A never published"
        );
        publish(registry, second, &key, WorkerReadinessState::Busy);
        let (owner, readiness, _) =
            worker_attempt_snapshot(registry, &key).expect("replaced");
        assert_eq!(owner, second, "the entry still names the successor");
        assert!(
            acp_readiness_is_ready(readiness),
            "the current owner publishes"
        );
        unregister_worker(&delivery, registry, &key, second);
    }
}
