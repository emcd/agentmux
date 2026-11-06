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

/// Atomically snapshots the registered worker's owner, readiness, and
/// stopping state. One acquisition covers all three fields, so a serving
/// observation is always attributed to the owner that produced it — never
/// another worker's target-scoped readiness treated as the waited attempt's
/// result — and the lifecycle eligibility is rechecked as part of the same
/// coherent observation the serving decision reads. A stop landing after
/// the snapshot is a later event, not a misattribution; an entry already
/// stopping at the snapshot is never reported serving.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn worker_attempt_snapshot(
    registry: &AsyncDeliveryRegistry,
    key: &AsyncWorkerKey,
) -> Option<(WorkerOwner, Option<WorkerReadinessState>, bool)> {
    let workers = registry.workers.lock().ok()?;
    let entry = workers.get(key)?;
    Some((entry.owner, entry.readiness, entry.stopping))
}

/// The outcome of a Pty bring-up election: either this pass installed the
/// worker or another pass already had.
///
/// Both variants name the attempt with the entry's own completion handle,
/// cloned under the same workers-lock acquisition that observed the entry
/// — so a loser that observed a winner retains the winner even if the
/// winner records and unregisters before the loser polls. No second
/// acquisition, no cache, and no misattribution: the waiter names exactly
/// the attempt it observed.
#[cfg(feature = "pty")]
#[derive(Debug)]
pub(in crate::relay::delivery) enum PtyElection {
    /// This pass won: the entry is newly installed and the worker remains
    /// to be spawned by the caller.
    Elected {
        owner: WorkerOwner,
        completion: Arc<AttemptCompletion>,
    },
    /// Another pass already owned the target: the observed attempt, named
    /// with its handle, so a winner that fails before adoption still
    /// reports its own cause through the loser's handle.
    Existing {
        owner: WorkerOwner,
        completion: Arc<AttemptCompletion>,
    },
}

/// Elects a Pty target's delivery worker, returning the elected attempt with
/// its completion handle under the SAME workers-lock acquisition as the
/// election itself.
///
/// The handle is born with the election: the entry field is assigned as the
/// entry is installed, so no interleaving — purge, replacement, unregister
/// — can land between naming the attempt and retaining its result. A loser
/// that finds an entry already installed names that entry's own handle
/// under the same acquisition instead of re-resolving it afterwards, so a
/// winner that records and unregisters in between still reports its own
/// cause through the loser. There is no second acquisition to split, and
/// nothing to purge afterwards: each attempt carries its own handle in its
/// own entry. Replaces `register_worker_if_absent` for the Pty bring-up
/// path only; the ACP and lazy-Direct paths keep the shared election.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) fn elect_pty_attempt(
    registry: &AsyncDeliveryRegistry,
    key: AsyncWorkerKey,
    sender: UnboundedSender<AsyncDeliveryTask>,
    pending: Arc<AtomicUsize>,
    bounded_acp_queue: bool,
) -> Result<PtyElection, RelayError> {
    let mut workers = registry.workers.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;
    if let Some(entry) = workers.get(&key) {
        return Ok(PtyElection::Existing {
            owner: entry.owner,
            completion: entry.completion.clone(),
        });
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
    Ok(PtyElection::Elected { owner, completion })
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
mod storms;
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
        let (owner, handle) = match elect_pty_attempt(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        )
        .expect("registry lock")
        {
            PtyElection::Elected { owner, completion } => (owner, completion),
            PtyElection::Existing { .. } => {
                panic!("a fresh target elects")
            },
        };
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
mod lost_election_tests {
    use super::super::reap::unregister_worker;
    use super::construction_completion_fixtures::*;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn elect(
        registry: &AsyncDeliveryRegistry,
        key: &AsyncWorkerKey,
    ) -> PtyElection {
        let (sender, _) =
            tokio::sync::mpsc::unbounded_channel::<AsyncDeliveryTask>();
        elect_pty_attempt(
            registry,
            key.clone(),
            sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        )
        .expect("registry lock")
    }

    /// A loser that observed the winner retains the winner's own cause:
    /// adopt#1 names nothing, the winner elects, the loser elects and
    /// names the same entry with its handle under the same acquisition —
    /// then the winner records and unregisters. The loser's handle still
    /// reads the recorded cause, and a replacement describes only itself.
    /// A split that re-resolves after the election (or drops the observed
    /// handle) leaves the loser with no cause here.
    #[test]
    fn observed_election_winner_retains_own_cause() {
        let delivery = test_delivery();
        let registry = delivery.async_registry();
        let key = key("construction-lost-election");
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "nothing registered before the winner elects"
        );
        // Winner elects and retains its handle as the first witness.
        let (winner, winner_handle) = match elect(registry, &key) {
            PtyElection::Elected { owner, completion } => (owner, completion),
            PtyElection::Existing { .. } => {
                panic!("a fresh target elects")
            },
        };
        // Loser elects while E lives: Existing names the same attempt with
        // the entry's own handle — no second acquisition to split.
        let (loser, loser_handle) = match elect(registry, &key) {
            PtyElection::Existing { owner, completion } => (owner, completion),
            PtyElection::Elected { .. } => {
                panic!("an occupied target reports the existing attempt")
            },
        };
        assert_eq!(loser, winner, "the loser names the winner");
        assert!(
            Arc::ptr_eq(&loser_handle, &winner_handle),
            "election and existing name one handle"
        );
        // Winner fails: the record strictly precedes the unregister, then
        // E is gone — adoption afterwards names nothing.
        assert!(
            record_construction_error(
                registry,
                &key,
                winner,
                cause("pty_spawn_failed"),
            ),
            "the winner's record lands on its own attempt"
        );
        unregister_worker(&delivery, registry, &key, winner);
        assert!(
            adopt_registered_attempt(registry, &key).is_none(),
            "the unregistered winner is gone"
        );
        // The loser never adopted through a second acquisition, yet
        // observes E's own cause through the retained handle.
        assert_eq!(
            loser_handle
                .read()
                .expect("the loser retains E's result")
                .code,
            "pty_spawn_failed",
            "the observed winner reports its own cause to the loser"
        );
        assert_eq!(
            winner_handle
                .read()
                .expect("the winner retains E's result")
                .code,
            "pty_spawn_failed",
            "the winner observes its own cause"
        );
        // Replacement describes only itself: a successor records its own
        // cause, and the loser's handle still reads the winner's.
        let second = install(registry, &key);
        assert_ne!(second, winner, "the test needs a distinct successor");
        assert!(
            record_construction_error(
                registry,
                &key,
                second,
                cause("pty_open_failed"),
            ),
            "the successor's record lands on its own attempt"
        );
        assert_eq!(
            loser_handle
                .read()
                .expect("the loser retains E's result")
                .code,
            "pty_spawn_failed",
            "the replacement neither describes nor blinds the loser"
        );
        let (_, successor_handle) =
            adopt_registered_attempt(registry, &key).expect("registered");
        assert_eq!(
            successor_handle.read().expect("successor recorded").code,
            "pty_open_failed",
            "the successor observes its own cause"
        );
        unregister_worker(&delivery, registry, &key, second);
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
        let (owner, readiness, stopping) =
            worker_attempt_snapshot(registry, &key).expect("registered");
        assert_eq!(owner, first, "readiness is attributed to A");
        assert!(
            acp_readiness_is_ready(readiness),
            "A serving is observable in the same snapshot"
        );
        assert!(!stopping, "a fresh registration is not stopping");
        // Replacement: the snapshot moves owner and readiness together —
        // never A's owner with B's readiness or vice versa.
        unregister_worker(&delivery, registry, &key, first);
        let second = install(registry, &key);
        let (owner, readiness, stopping) =
            worker_attempt_snapshot(registry, &key).expect("re-registered");
        assert_eq!(owner, second, "the snapshot follows the replacement");
        assert!(!stopping, "the replacement is not stopping either");
        assert!(
            !acp_readiness_is_ready(readiness),
            "the fresh registration carries no readiness A published"
        );
        unregister_worker(&delivery, registry, &key, second);
    }
}
