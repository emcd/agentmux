//! Telling a target's consumer that peeking is worth its while.
//!
//! The doorbell is the one signal that runs from the relay toward a consumer,
//! and it is deliberately the weakest thing that could work. It carries no data
//! and takes no custody: everything a consumer acts on it reads back out of the
//! mailbox for itself. So a doorbell that never arrives costs a delay and
//! nothing else, and the bounded poll every delivery worker already runs is what
//! bounds that delay.
//!
//! **The relay never learns who it rang.** A doorbell is an opaque closure the
//! consumer side injects, in the same shape as the readiness notifier a
//! transport is handed today — which is what keeps this a new event for an
//! existing mechanism rather than a second signalling path with its own
//! failure modes.
//!
//! One doorbell per target, replaced rather than accumulated: a generation
//! registers its own as it is built, and **only a later registration ever
//! displaces it.** Neither the reap nor the fenced replacement clears one, which
//! is what makes this safe rather than merely untidy — both of those run behind
//! the target they act on, so a successor can already have registered by the
//! time either reaches the ledger, and a clear would take the successor's
//! doorbell with nothing left to put one back. A registration therefore outlives
//! the generation that made it, until the next generation supersedes it or the
//! process ends. Nothing rings a superseded one: an entry reaches a mailbox only
//! through a worker's own intake, so a target with no worker has nothing to
//! enqueue.
//!
//! Nothing here is persisted, and a relay that restarts rings nothing until the
//! generations that come back register again — the mailbox contents, not the
//! doorbell, are what a new generation's first peek recovers.

use std::sync::Arc;

use crate::protocol::identity::DeliveryTargetId;

use super::super::ledger::lock_ledger;
use super::addressing::target_key;

/// What the relay rings.
///
/// No arguments, no return, no data. A doorbell that could carry an entry would
/// be a second delivery path beside the mailbox, and a consumer could come to
/// depend on having been rung rather than on what a `peek` reports. All this can
/// say is that peeking may now be worth doing.
///
/// A closure rather than the consumer's handle, so the relay holds no type
/// belonging to the side it is signalling and learns nothing by ringing. What
/// the closure rings in production is
/// [`DeliveryDoorbell`](crate::protocol::DeliveryDoorbell), the neutral handle
/// the delivery-loop executor waits on — but that is the registrant's business
/// and deliberately not visible here.
pub(in crate::relay) type Doorbell = Arc<dyn Fn() + Send + Sync>;

/// Records the doorbell the relay rings for `target`, replacing whatever the
/// previous generation left there.
///
/// **The only write to this map, and the only removal from it.** Nothing else
/// takes a registration away, which is what keeps the last registrant the one
/// that gets rung no matter how the calls interleave with a teardown.
///
/// Replacement rather than refusal, and not checked against the target's active
/// consumer generation, which every operation that moves an entry *is* checked
/// against. The asymmetry is deliberate: those operations decide what a target
/// has delivered, and a stale caller reaching one would corrupt that record,
/// whereas the worst a stale registration can do is leave a live consumer
/// waiting for a doorbell that goes to a handle nobody holds — which is the
/// missed notification the bounded poll already backstops. Gating registration
/// would buy nothing and would put a generation check on the one path whose
/// contract says correctness must not depend on it — and the check available to
/// gate it with does not presently discriminate anything, since no worker claims
/// a consumer generation until the delivery-loop executors arrive.
///
/// A ledger that will not lock registers nothing rather than failing the caller.
/// The generation being built has no worse outcome available to it: its consumer
/// will poll, which is what it would have done for a notification that was rung
/// and missed.
pub(in crate::relay) fn register_doorbell(target: &DeliveryTargetId, doorbell: Doorbell) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    state.doorbells.insert(target_key(target), doorbell);
}

/// A doorbell rings when a peek that would have come back empty would now come
/// back with something, and not otherwise.
///
/// Inline for the reason given on the peek block: the ledger is a process-global
/// behind a crate-private lock, and widening these operations to reach them from
/// `tests/` would publish the delivery ledger itself.
///
/// One test because the property is one claim with a boundary on each side. A
/// doorbell that rang on every enqueue would wake a consumer that has already
/// been told, and one that rang only on the first would leave a target silent
/// for every later run — and the case that tells the two apart from the obvious
/// "the mailbox went from empty to non-empty" reading is neither of those: an
/// entry can arrive into an empty mailbox and still leave a peek returning
/// nothing, because the position ahead of it has been admitted and not yet
/// filled.
///
/// It closes on the registration's lifetime, which is the same claim from the
/// other end: a ring reaches whoever registered last, and a reap arriving behind
/// a successor is not a registration.
#[cfg(test)]
mod doorbell_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::super::terminal::terminalize;
    use super::super::enqueue::enqueue;
    use super::super::fixtures::{admit_only, mail, place, target, task};
    use super::super::reap::reap_target;
    use super::*;

    #[test]
    fn a_doorbell_rings_when_the_head_becomes_peekable_and_not_otherwise() {
        let namespace = "mbx-doorbell";
        let rings = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&rings);
        register_doorbell(
            &target(namespace),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );

        // Nothing was peekable and now something is.
        place(namespace, "mbx-doorbell-1", 1, mail("body"));
        assert_eq!(
            rings.load(Ordering::SeqCst),
            1,
            "the first entry into an empty mailbox rings"
        );

        // The head is already peekable, so a consumer that has been told once has
        // been told. Ringing here would wake it for a run it can already see.
        place(namespace, "mbx-doorbell-2", 1, mail("body"));
        assert_eq!(
            rings.load(Ordering::SeqCst),
            1,
            "an entry behind an already-peekable head rings nothing"
        );

        // The cursor moving is not an arrival. Entry two was behind the head and
        // is now at it, but no consumer needs telling about a position that was
        // in the mailbox before.
        terminalize("mbx-doorbell-1");
        place(namespace, "mbx-doorbell-3", 1, mail("body"));
        assert_eq!(
            rings.load(Ordering::SeqCst),
            1,
            "an entry arriving behind a head the cursor moved onto rings nothing"
        );

        // The case that separates this from "the mailbox went from empty to
        // non-empty". Both remaining entries drain, then position four is
        // admitted and left unfilled while five is enqueued behind it: the
        // mailbox is no longer empty, and a peek still returns nothing, because
        // the run has to start at the cursor.
        terminalize("mbx-doorbell-2");
        terminalize("mbx-doorbell-3");
        admit_only(namespace, "mbx-doorbell-4", 1);
        admit_only(namespace, "mbx-doorbell-5", 1);
        enqueue(&task(namespace, "mbx-doorbell-5"), mail("body")).expect("enqueue");
        assert_eq!(
            rings.load(Ordering::SeqCst),
            1,
            "an entry that fills a position behind an unfilled one rings nothing"
        );
        enqueue(&task(namespace, "mbx-doorbell-4"), mail("body")).expect("enqueue");
        assert_eq!(
            rings.load(Ordering::SeqCst),
            2,
            "the entry that finally makes the head peekable rings"
        );

        // A reap arriving behind a successor must not take the successor's
        // doorbell. This is the order the live teardown actually produces: a
        // worker unregisters, releasing the registry lock before it reaches the
        // ledger, and a successor can be elected and have registered its own
        // doorbell inside that window — so the reap below runs *after* the
        // registration it must not disturb. The consumer-generation naming does
        // not cover this case and cannot be made to: until the executors claim a
        // generation every target answers `None`, so the reap matches and
        // proceeds. What covers it is that nothing but a registration ever
        // displaces a registration.
        terminalize("mbx-doorbell-4");
        terminalize("mbx-doorbell-5");
        let successor_rings = Arc::new(AtomicUsize::new(0));
        let successor_counter = Arc::clone(&successor_rings);
        register_doorbell(
            &target(namespace),
            Arc::new(move || {
                successor_counter.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert!(
            reap_target(&target(namespace), None).is_ok(),
            "the fixture target is unclaimed, so a reap naming nothing is its reap"
        );
        place(namespace, "mbx-doorbell-6", 1, mail("body"));
        assert_eq!(
            successor_rings.load(Ordering::SeqCst),
            1,
            "a reap running behind a successor leaves the successor's doorbell in place"
        );
        // And the reap did not resurrect the one the successor replaced, which is
        // what would happen if removal were made conditional on something the
        // registration did not overwrite.
        assert_eq!(
            rings.load(Ordering::SeqCst),
            2,
            "the registration the successor replaced stays replaced"
        );
    }
}
