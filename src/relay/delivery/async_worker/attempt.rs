//! One construction attempt's completion, and the waiters observing it.
//!
//! A worker unregisters on first-generation construction failure, taking its
//! registry entry with it. A bring-up prime wait polling after the unregister
//! would find nothing and report a generic timeout instead of the transport's
//! own code. Every registry entry therefore carries its attempt's completion
//! handle (`AsyncWorkerEntry::completion`): waiters clone the handle under
//! the same workers-lock acquisition that observes or elects the entry, so
//! the result outlives the entry, the unregister, and any replacement.
//!
//! There is no completions table, and deliberately so. A table keyed by
//! attempt needs deletion for hygiene (supersession purge, teardown purge,
//! success clearing), and every deletion path is a lifetime hazard for a
//! waiter holding that attempt: peeking a map entry is not a lifetime
//! guarantee when registration deletes the entry. An entry-stored handle
//! has no deletion path at all — a replacement installs its own handle in
//! its own entry, and a waiter's clone is unaffected by anything any other
//! attempt does. Owner-keying (not target-keying) is what keeps replacements
//! from cross-describing attempts: two waiters on one attempt read the
//! same handle, and an old attempt's handle is invisible to a replacement
//! holding a different owner.
//!
//! Lock discipline follows from the shape: every waiter path needs exactly
//! one workers-lock acquisition (clone the handle, or snapshot owner and
//! readiness together). The record path needs one acquisition to name its
//! handle plus the handle's own mutex. Splitting any of these into two
//! acquisitions is the defect class this module exists to prevent, and it
//! is review-visible as such: no waiter path spans more than one
//! acquisition, because no waiter path has anything to do after the first
//! except read through the clone it already holds.
//!
//! Gated with the Pty feature: Pty is the only transport that records here
//! today (the one whose worker unregisters on construction failure), and an
//! unused contract in every other build would be dead code.

use std::sync::{Arc, Mutex, atomic::AtomicUsize};

use tokio::sync::mpsc::UnboundedSender;

use crate::relay::{AsyncDeliveryTask, RelayError, relay_error};
use crate::transports::{WorkerFailureReason, WorkerReadinessState};

use super::registry::{
    AsyncDeliveryRegistry, AsyncWorkerEntry, AsyncWorkerKey, WorkerOwner,
};

/// One construction attempt's immutable result, shared with every waiter
/// that adopted or elected the attempt. Stored in the registry entry and
/// cloned out under the workers lock; waiters never touch it through any
/// other path.
#[cfg(feature = "pty")]
#[derive(Debug, Default)]
pub(in crate::relay) struct AttemptCompletion {
    cause: Mutex<Option<WorkerFailureReason>>,
}

#[cfg(feature = "pty")]
impl AttemptCompletion {
    fn record(&self, failure: WorkerFailureReason) {
        if let Ok(mut cause) = self.cause.lock() {
            *cause = Some(failure);
        }
    }

    /// Reads the recorded cause without removing it. Every waiter holding
    /// the attempt's handle observes the same immutable result.
    pub(in crate::relay::delivery) fn read(
        &self,
    ) -> Option<WorkerFailureReason> {
        self.cause.lock().ok()?.clone()
    }
}

/// Records a transport construction failure into its attempt's completion.
/// One acquisition names the handle — the entry's own, checked against the
/// recording owner so a stale worker can never attribute to its successor —
/// and the record lands after the lock is dropped. No deletion path exists
/// for a stored completion, so nothing can invalidate the clone between
/// the two steps.
///
/// Returns whether the record landed. A worker records strictly before it
/// unregisters, so in production this is always true; the `false` arm
/// exists for a teardown that won the race, where attributing to a
/// replacement would be the defect rather than the fix. Callers that log
/// the attempt as recorded must gate on this: a logged attempt whose
/// record was skipped is unobservable by design. Recording twice for one
/// attempt overwrites: a worker builds once per attempt, so a second
/// record can only restate.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn record_construction_error(
    registry: &AsyncDeliveryRegistry,
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
    failure: WorkerFailureReason,
) -> bool {
    let completion = registry.workers.lock().ok().and_then(|workers| {
        let entry = workers.get(key)?;
        (entry.owner == owner).then(|| entry.completion.clone())
    });
    if let Some(completion) = completion {
        completion.record(failure);
        return true;
    }
    false
}

/// Atomically adopts the currently registered worker as the waited attempt,
/// returning its owner with its completion handle. A single workers-lock
/// acquisition covers both the observation and the handle clone, so no
/// unregister can land between seeing the entry and naming the attempt —
/// the lookup race this replaces is unrepresentable rather than merely
/// unlikely.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn adopt_registered_attempt(
    registry: &AsyncDeliveryRegistry,
    key: &AsyncWorkerKey,
) -> Option<(WorkerOwner, std::sync::Arc<AttemptCompletion>)> {
    let workers = registry.workers.lock().ok()?;
    let entry = workers.get(key)?;
    Some((entry.owner, entry.completion.clone()))
}

/// Atomically snapshots the registered worker's owner and readiness. One
/// acquisition covers both fields, so a serving observation is always
/// attributed to the owner that produced it — never another worker's
/// target-scoped readiness treated as the waited attempt's result.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn worker_attempt_snapshot(
    registry: &AsyncDeliveryRegistry,
    key: &AsyncWorkerKey,
) -> Option<(WorkerOwner, Option<WorkerReadinessState>)> {
    let workers = registry.workers.lock().ok()?;
    let entry = workers.get(key)?;
    Some((entry.owner, entry.readiness))
}

/// Elects a Pty target's delivery worker, returning the elected attempt with
/// its completion handle under the SAME workers-lock acquisition as the
/// election itself.
///
/// The handle is born with the election: the entry field is assigned as the
/// entry is installed, so no interleaving — purge, replacement, unregister
/// — can land between naming the attempt and retaining its result. There
/// is no second acquisition to split, and nothing to purge afterwards:
/// each attempt carries its own handle in its own entry. Replaces
/// `register_worker_if_absent` for the Pty bring-up path only; the ACP and
/// lazy-Direct paths keep the shared election.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn elect_pty_attempt(
    registry: &AsyncDeliveryRegistry,
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> Result<Option<(WorkerOwner, Arc<AttemptCompletion>)>, RelayError> {
    let mut workers = registry.workers.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    if workers.contains_key(&key) {
        return Ok(None);
    }
    let owner = WorkerOwner::mint();
    let completion = Arc::new(AttemptCompletion::default());
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
            stopping: false,
            fail_stopped: false,
            consumer_generation: None,
            #[cfg(feature = "pty")]
            completion: completion.clone(),
        },
    );
    Ok(Some((owner, completion)))
}
/// Attempt-handle semantics: adoption atomicity, waiter lifetime across
/// unregister and replacement, and owner-attributed snapshots.
/// Attempt-handle semantics: adoption atomicity, waiter lifetime across
/// unregister and replacement, and owner-attributed snapshots.
///
/// Shared fixtures for the one-test modules below: each test lives in its
/// own inline module (one `#[test]` per block), because the contract under
/// test is `pub(in crate::relay::delivery)` with no public surface — the
/// same terms as the `terminal.rs` inline tests — and widening visibility
/// to reach these paths from `tests/` would publish the worker registry.
/// Test registries are per-test `DeliveryContext` handles, so the harness
/// running tests in parallel threads needs no namespace discipline beyond
/// readability.
///
/// The sequential tests drive the failure traces from the review findings
/// directly: each operation holds its lock for its whole body, so the
/// adversarial order is fully determined. They pin that the handle a
/// waiter holds always names the recorded cause — the property the old
/// map-and-purge design violated. The two storm tests add continuous
/// replacement pressure plus log-gated assertions: every logged adoption
/// or snapshot is checked, so a split acquisition has to survive the
/// collision rate to pass; the single-acquisition primitives hold
/// regardless.
#[cfg(all(test, feature = "pty"))]
mod construction_completion_fixtures {
    use super::super::registry::{
        AsyncDeliveryRegistry, AsyncWorkerKey, WorkerOwner, register_worker,
    };
    use crate::relay::AsyncDeliveryTask;
    use crate::transports::WorkerFailureReason;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub(super) fn key(namespace: &str) -> AsyncWorkerKey {
        AsyncWorkerKey {
            runtime_directory: PathBuf::from("/construction-test"),
            namespace: namespace.to_string(),
            target_session: "target".to_string(),
        }
    }

    pub(super) fn cause(code: &str) -> WorkerFailureReason {
        WorkerFailureReason {
            code: code.to_string(),
            reason: format!("{code} reason"),
        }
    }

    pub(super) fn test_delivery() -> Arc<crate::relay::DeliveryContext> {
        Arc::new(crate::relay::DeliveryContext::new(Default::default()))
    }

    pub(super) fn install(
        registry: &AsyncDeliveryRegistry,
        key: &AsyncWorkerKey,
    ) -> WorkerOwner {
        let (sender, _) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        register_worker(
            registry,
            key.clone(),
            sender,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            false,
        )
    }
}
#[cfg(all(test, feature = "pty"))]
mod elected_handle_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// The election binds its handle to the attempt: the handle the
    /// election returns is the entry's own, so a cause recorded for the
    /// elected owner is observable through it. A mutation returning a
    /// detached handle (or fetching one in a later acquisition) fails
    /// here deterministically — no concurrency required, because the
    /// property is about identity, not timing.
    #[test]
    fn elected_handle_is_the_entry_handle() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-elect-identity");
        let (sender, _) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        let (owner, handle) = elect_pty_attempt(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        )
        .expect("registry lock")
        .expect("the target is free");
        record_construction_error(
            registry,
            &key,
            owner,
            cause("pty_spawn_failed"),
        );
        let (_, adopted) =
            adopt_registered_attempt(registry, &key).expect("registered");
        assert!(
            Arc::ptr_eq(&handle, &adopted),
            "election, adoption, and the entry name one handle"
        );
        assert_eq!(
            handle.read().expect("recorded").code,
            "pty_spawn_failed",
            "the elected handle carries the recorded cause"
        );
        unregister_worker(&delivery, registry, &key, owner);
        assert!(
            handle.read().is_some(),
            "the handle outlives the unregistered entry"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod adopt_boundary_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;

    /// The adoption boundary: a waiter that adopted attempt A keeps A's
    /// handle across A's unregister and record. The old owner-id re-read
    /// lost the cause here — it re-read None after the unregister and
    /// never learned A existed — while the handle survives because its
    /// lifetime follows the waiter, not the entry.
    #[test]
    fn adopted_handle_survives_unregister_before_record() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-adopt-boundary");
        let elected = install(registry, &key);
        // Atomic adopt: one acquisition names the owner and clones the
        // handle together — no unregister can land between the two.
        let (owner, handle) = adopt_registered_attempt(registry, &key)
            .expect("the registered attempt adopts");
        assert_eq!(owner, elected, "adopt names the elected attempt");
        // A's whole failure plays out after the adopt: record strictly
        // precedes unregister in the failing worker.
        record_construction_error(
            registry,
            &key,
            owner,
            cause("pty_spawn_failed"),
        );
        unregister_worker(&delivery, registry, &key, owner);
        assert!(
            worker_attempt_snapshot(registry, &key).is_none(),
            "the entry is gone, which is what stranded the old re-read"
        );
        assert_eq!(
            handle.read().expect("the handle retains A's result").code,
            "pty_spawn_failed",
            "the adopted handle observes the cause despite the unregister"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod delayed_waiter_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;

    /// A delayed waiter survives an intervening replacement failure: W
    /// holds A's handle while B registers, fails, and unregisters. W
    /// still observes A's immutable result, and B's result is separately
    /// intact. There is no purge to blind W — replacements install their
    /// own handles in their own entries.
    #[test]
    fn delayed_waiter_survives_intervening_replacement_failure() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-delayed-waiter");
        let first = install(registry, &key);
        // W adopts attempt A before anything fails.
        let (_, waiter) = adopt_registered_attempt(registry, &key)
            .expect("W adopts attempt A");
        assert!(
            waiter.read().is_none(),
            "nothing recorded yet for the adopted attempt"
        );
        // Record strictly precedes unregister in the failing worker: the
        // build's failure closure records before the error propagates to
        // the unregister. The test mirrors production order, since a
        // record after the unregister would name an attempt the registry
        // no longer holds.
        record_construction_error(
            registry,
            &key,
            first,
            cause("pty_spawn_failed"),
        );
        unregister_worker(&delivery, registry, &key, first);
        // B's whole lifecycle runs while W is descheduled: register, fail,
        // unregister.
        let second = install(registry, &key);
        assert_ne!(first, second, "the test needs two distinct attempts");
        record_construction_error(
            registry,
            &key,
            second,
            cause("pty_open_failed"),
        );
        unregister_worker(&delivery, registry, &key, second);
        assert!(
            worker_attempt_snapshot(registry, &key).is_none(),
            "no worker remains, which is when the old take lost everything"
        );
        assert_eq!(
            waiter.read().expect("W retains A's result").code,
            "pty_spawn_failed",
            "the replacement neither describes nor blinds the waiter"
        );
    }
}
#[cfg(all(test, feature = "pty"))]
mod adopt_successor_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;

    /// Adopting after a replacement names the successor, never the
    /// superseded attempt: the waiter polls the live attempt's outcome,
    /// not the old one's.
    #[test]
    fn adopt_after_replacement_names_the_successor() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-adopt-successor");
        let first = install(registry, &key);
        record_construction_error(
            registry,
            &key,
            first,
            cause("pty_spawn_failed"),
        );
        unregister_worker(&delivery, registry, &key, first);
        let second = install(registry, &key);
        let (owner, handle) = adopt_registered_attempt(registry, &key)
            .expect("the successor adopts");
        assert_eq!(owner, second, "adopt names the live attempt");
        assert!(
            handle.read().is_none(),
            "the successor has recorded nothing yet"
        );
        unregister_worker(&delivery, registry, &key, second);
    }
}
#[cfg(all(test, feature = "pty"))]
mod snapshot_attribution_tests {
    use super::super::reap::unregister_worker;
    use super::super::registry::{
        acp_readiness_is_ready, set_worker_readiness,
    };
    use super::construction_completion_fixtures::*;
    use super::*;

    /// Owner and readiness move together in one snapshot: a serving
    /// observation is always attributed to the owner that produced it.
    /// A waiter holding A distinguishes B-serving from A-serving, so no
    /// clear can fire on another worker's readiness.
    #[test]
    fn snapshot_attributes_serving_to_its_owner() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-snapshot-attribution");
        let first = install(registry, &key);
        assert_eq!(
            worker_attempt_snapshot(registry, &key)
                .expect("registered")
                .0,
            first,
            "the snapshot names the registered owner"
        );
        set_worker_readiness(
            registry,
            key.namespace.as_str(),
            key.runtime_directory.as_path(),
            key.target_session.as_str(),
            WorkerReadinessState::Available,
        );
        let (owner, readiness) =
            worker_attempt_snapshot(registry, &key).expect("registered");
        assert_eq!(owner, first, "readiness is attributed to A");
        assert!(
            acp_readiness_is_ready(readiness),
            "A serving is observable in the same snapshot"
        );
        // Replacement: the snapshot moves owner and readiness together —
        // never A's owner with B's readiness or vice versa.
        unregister_worker(&delivery, registry, &key, first);
        let second = install(registry, &key);
        let (owner, readiness) =
            worker_attempt_snapshot(registry, &key).expect("re-registered");
        assert_eq!(owner, second, "the snapshot follows the replacement");
        assert!(
            !acp_readiness_is_ready(readiness),
            "the fresh registration carries no readiness A published"
        );
        unregister_worker(&delivery, registry, &key, second);
    }
}
#[cfg(all(test, feature = "pty"))]
mod adopt_storm_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;
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
    use super::super::reap::unregister_worker;
    use super::super::register_worker_with_readiness;
    use super::construction_completion_fixtures::*;
    use super::*;
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
                        if let Some((snapshot_owner, readiness)) =
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
