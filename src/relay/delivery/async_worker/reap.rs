//! Ending a worker's registration, and reclaiming what that registration owned.
//!
//! Registration is an election, so an entry leaving the registry is the moment a
//! target stops being anybody's. That makes this the one place a target's
//! mailbox and cursor can be reclaimed without racing a consumer: a worker whose
//! generation is fenced and rebuilt in place keeps its registration across the
//! replacement and never arrives here, so reaching here means the target itself
//! is going rather than changing hands.

use crate::protocol::identity::{ConsumerGenerationId, DeliveryTargetId};
use crate::relay::delivery::admission::reap_target;

use super::registry::{AsyncWorkerKey, WorkerOwner, async_delivery_registry};

/// Records the consumer generation a worker has claimed for its target, so the
/// reap can name it.
///
/// Ownership-checked for the same reason every other mutation on an entry is: a
/// worker that lost its registration must not stamp its generation onto a
/// successor's entry, which would make the successor's reap give up a generation
/// it never held.
///
/// **Nothing calls this, and the reap below cannot work until something does.**
/// Every worker claims a consumer generation as it builds one, so the ledger
/// records each live target as held; an entry that never carries what its worker
/// claimed makes the reap name `None` against that record, which the ledger
/// refuses. The generation is then never given up and no successor can claim the
/// target.
#[allow(dead_code)]
pub(in crate::relay::delivery) fn bind_worker_consumer_generation(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
    generation: ConsumerGenerationId,
) {
    if let Ok(mut workers) = async_delivery_registry().workers.lock()
        && let Some(entry) = workers.get_mut(key)
        && entry.owner == owner
    {
        entry.consumer_generation = Some(generation);
    }
}

/// Removes this target's registry entry, but only if `owner` still holds it,
/// and reaps the target's mailbox with it.
///
/// The ownership check is what keeps an exiting worker from deleting a
/// successor's entry. A worker that lost a registration race, or whose entry was
/// already replaced, finds a different owner here and leaves it alone — removing
/// it would drop the only sender for a live worker and silently strand every
/// subsequent send to that target. It reaps nothing in that case either, for the
/// same reason: the target it would be reclaiming is not the one it lost.
///
/// The ledger is reached after the registry lock is released, not under it.
/// Nesting the two would introduce an ordering between locks that nothing else
/// in this subsystem observes, and one inverted acquisition anywhere would
/// deadlock the relay. The window that opens instead is the very case the reap's
/// naming exists for: a consumer that claims this target between the removal and
/// the reap holds a generation this entry never named, so the reap refuses
/// rather than reclaiming a mailbox somebody is already serving.
pub(in crate::relay::delivery) fn unregister_worker(key: &AsyncWorkerKey, owner: WorkerOwner) {
    let Some(held) = remove_owned_entry(key, owner) else {
        return;
    };
    let target = DeliveryTargetId::new(
        key.namespace.as_str(),
        key.runtime_directory.as_path(),
        key.target_session.as_str(),
    );
    // A refusal is the correct answer rather than a failure to handle: it says
    // some other consumer holds the target now, so there is nothing here to give
    // up. `Retained` is the reading worth an operator's attention — a target
    // still holding entries once its worker has gone means the teardown path
    // that owed those entries their outcomes did not run. The reap records that
    // and deliberately resolves nothing, because it knows the entries by id and
    // could not report a single one of them.
    let _ = reap_target(&target, held);
}

/// Removes the entry if `owner` holds it, reporting the consumer generation it
/// carried.
///
/// The two-level option is flattened deliberately: the outer `Some` means "this
/// caller's entry was removed", and the inner value is what it held. Reported
/// as one value so no caller can mistake "nothing was removed" for "an entry
/// holding no generation was removed" — the first must reap nothing, and the
/// second must reap a target nobody claimed.
fn remove_owned_entry(
    key: &AsyncWorkerKey,
    owner: WorkerOwner,
) -> Option<Option<ConsumerGenerationId>> {
    let mut workers = async_delivery_registry().workers.lock().ok()?;
    if workers.get(key).is_none_or(|entry| entry.owner != owner) {
        return None;
    }
    Some(
        workers
            .remove(key)
            .and_then(|entry| entry.consumer_generation),
    )
}

/// A registration leaving takes its target's mailbox with it, and a registration
/// that was never this caller's takes nothing.
///
/// Inline because the registry seam is crate-private by design — `register_worker`
/// is itself `#[cfg(test)]`-only — so no public interface reaches this path, and
/// widening one to reach it from `tests/` would publish the worker registry.
///
/// One test because the two halves are the same claim from both sides. A reap
/// that fires for a non-owner would reclaim a live worker's mailbox, and one
/// that fails to fire for the owner leaves the record it exists to reclaim; a
/// suite asserting only the first would pass against a reap wired to nothing at
/// all.
#[cfg(test)]
mod worker_reap_tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use crate::configuration::SessionType;
    use crate::protocol::mailbox::MailboxPayload;
    use crate::relay::delivery::admission::{
        AdmissionTargetKey, GenerationRejection, TargetReap, admit, claim_consumer_generation,
        enqueue, reap_target, terminalize,
    };

    use super::super::registry::{build_worker_key, register_worker};
    use super::*;

    const NAMESPACE: &str = "worker-reap-test";
    const TARGET: &str = "target";

    #[test]
    fn unregistering_an_owned_entry_reaps_its_target_and_a_foreign_one_reaps_nothing() {
        let key = build_worker_key(NAMESPACE, runtime_directory().as_path(), TARGET);
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let owner = register_worker(key.clone(), sender, Arc::new(AtomicUsize::new(0)), false);
        let generation =
            claim_consumer_generation(&target()).expect("the test target is unclaimed");
        bind_worker_consumer_generation(&key, owner, generation);
        admit("worker-reap-1", admission_key(), SessionType::Tmux, 1).expect("admit");
        enqueue(
            &Arc::new(queued_task("worker-reap-1")),
            MailboxPayload::Raw {
                content: "body".to_string(),
                append_enter: true,
            },
        )
        .expect("enqueue");
        terminalize("worker-reap-1");

        // A second worker's owner, which never held this entry. Unregistering
        // under it must leave both the registration and the target alone —
        // otherwise a worker that lost a registration race would reclaim the
        // mailbox its successor is serving.
        let (other_sender, _other_receiver) = tokio::sync::mpsc::unbounded_channel();
        let stranger = register_worker(
            build_worker_key(NAMESPACE, runtime_directory().as_path(), "other-target"),
            other_sender,
            Arc::new(AtomicUsize::new(0)),
            false,
        );
        unregister_worker(&key, stranger);
        assert_eq!(
            claim_consumer_generation(&target()),
            Err(GenerationRejection::AlreadyHeld { active: generation }),
            "a non-owner's unregister gives up nothing"
        );

        // The owner's unregister is the reap: the generation is given up and the
        // mailbox goes with the registration.
        unregister_worker(&key, owner);
        let next =
            claim_consumer_generation(&target()).expect("the reaped target is free to claim again");
        assert!(
            next.value() > generation.value(),
            "the reap left the sequence behind rather than restarting it"
        );
        // And it really was the reap that ran, not merely a release: a second
        // reap naming the current holder finds nothing left to reclaim, which it
        // could not report if the first had left the mailbox in place holding a
        // cursor.
        assert_eq!(
            reap_target(&target(), Some(next)),
            Ok(TargetReap::Reclaimed),
            "the target the reap left behind holds nothing"
        );
    }

    fn runtime_directory() -> std::path::PathBuf {
        Path::new("/nonexistent").join(NAMESPACE)
    }

    fn target() -> DeliveryTargetId {
        DeliveryTargetId::new(NAMESPACE, runtime_directory().as_path(), TARGET)
    }

    fn admission_key() -> AdmissionTargetKey {
        AdmissionTargetKey::new(NAMESPACE, runtime_directory().as_path(), TARGET)
    }

    /// The send this test's entry answers for.
    ///
    /// Only its message id and target matter here: the reap is asserted on what
    /// it leaves in the ledger, not on anything reported to a sender.
    fn queued_task(message_id: &str) -> crate::relay::AsyncDeliveryTask {
        crate::relay::AsyncDeliveryTask {
            admitted: true,
            bundle: crate::configuration::BundleConfiguration {
                schema_version: crate::configuration::BUNDLE_SCHEMA_VERSION.to_string(),
                bundle_name: NAMESPACE.to_string(),
                autostart: false,
                groups: Vec::new(),
                members: Vec::new(),
            },
            sender_namespace: NAMESPACE.to_string(),
            sender: crate::configuration::BundleMember {
                id: "sender".to_string(),
                name: None,
                working_directory: None,
                target: crate::configuration::TargetConfiguration::Ui,
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
            runtime_directory: runtime_directory(),
            payload_mode: crate::transports::DeliveryPayloadMode::RawInput,
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
            sender_return_route: None,
        }
    }
}
