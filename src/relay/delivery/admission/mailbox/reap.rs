//! Reclaiming a target's mailbox when its registration goes away.
//!
//! One event, not two. Giving up the target's ownership and reclaiming what it
//! held have to happen under a single acquisition of the ledger lock, because a
//! release that returned before the reclamation would leave a window for a new
//! consumer to claim the target and begin serving a mailbox that is then
//! reclaimed underneath it. So this is the whole of the reap, and there is no
//! bare release beside it.
//!
//! **The generation being reaped is named, and a reap that does not name the
//! incumbent reaps nothing.** A reap runs behind the target it is reaping — the
//! registration is removed first, and the ledger is reached afterwards — so a
//! reap arriving after some other consumer has claimed the target is a reachable
//! ordering rather than a defensive hypothetical. Naming is what makes that case
//! a refusal instead of the silent theft of a live consumer's mailbox.
//!
//! **The naming does not protect anything yet, and nothing added here may lean
//! on it until it does.** No worker claims a consumer generation before the
//! delivery-loop executors arrive, so every target answers `None`, every reap
//! names `None`, and the check above matches for a late reap exactly as it does
//! for a timely one. What the window currently costs is nothing, because the
//! only state the reap takes is the successor's not-yet-used mailbox — recreated
//! on its first enqueue, from a sequence the reap deliberately leaves behind.
//! That is a property of what is reaped, not of the check, and it stops holding
//! the moment something the successor cannot rebuild is reaped alongside it.
//! Whatever is added here has to survive a reap arriving behind a live
//! successor on its own terms.

use serde_json::json;

use crate::protocol::identity::{ConsumerGenerationId, DeliveryTargetId};
use crate::runtime::inscriptions::emit_inscription;

use super::super::ledger::lock_ledger;
use super::addressing::target_key;
use super::generation::GenerationRejection;

const INSCRIPTION_MAILBOX_REAPED: &str = "relay.delivery.mailbox.reaped";

/// What a reap did to the target's mailbox.
///
/// The generation is given up either way. What varies is whether the mailbox
/// itself could go with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum TargetReap {
    /// Nothing was left to lose, so the mailbox and its cursor went with the
    /// registration.
    Reclaimed,
    /// Entries were still held for this target, so the mailbox stayed and only
    /// the ownership was given up.
    ///
    /// Deliberately not a reclamation. The reap knows these entries by id and
    /// not as the tasks that produced them, so it could terminalize them but
    /// could not report a single one — every sender waiting on an outcome would
    /// get silence, which is worse than the record it was cleaning up. They are
    /// owed outcomes by the teardown path that holds their tasks, and a reap
    /// that finds them is evidence that path did not run rather than an
    /// invitation to finish its work for it.
    Retained { entries_held: usize },
}

/// Gives up a target's consumer generation and reclaims what its mailbox holds.
///
/// The monotonic sequence itself is untouched, which is the point of the whole
/// arrangement: the high-water mark stays behind so a target recreated under the
/// same session name continues the sequence instead of restarting it, and an
/// identifier held from before this teardown cannot match one issued after.
/// Everything a mailbox *is* — its slots, its cursor, the positions it retired,
/// the units it acknowledged, the next position it would hand out — goes.
///
/// `outgoing` is the generation the caller believes it holds, and `None` is a
/// real answer rather than a wildcard: a caller that never claimed one matches a
/// target nobody has claimed, and is refused by a target somebody has.
pub(in crate::relay) fn reap_target(
    target: &DeliveryTargetId,
    outgoing: Option<ConsumerGenerationId>,
) -> Result<TargetReap, GenerationRejection> {
    let Ok(mut state) = lock_ledger() else {
        return Err(GenerationRejection::LedgerUnavailable);
    };
    let key = target_key(target);
    let active = state.generations.get(&key).and_then(|held| held.active);
    if active != outgoing {
        return Err(GenerationRejection::NotActive { active });
    }
    if let Some(generations) = state.generations.get_mut(&key) {
        generations.active = None;
    }
    // The doorbell is deliberately left alone, and this is the one piece of a
    // target's state the reap does not take.
    //
    // A reap runs *behind* the target it reaps — the registration is removed
    // first and the ledger is reached afterwards — so a successor can be elected,
    // spawned, and have registered its own doorbell before this call acquires the
    // lock. The naming above does not cover that: it compares consumer
    // generations, and until the delivery-loop executors claim one every target
    // answers `None`, so a late reap matches and proceeds. Removing here would
    // therefore take the successor's registration, and nothing would put one
    // back — the successor registers once, as it is built. It would be poll-only
    // for the rest of its life.
    //
    // Leaving it costs one small closure per target identity the process has
    // served, which is the same bound `generations` already carries and for a
    // similar reason. Nothing rings it: an entry reaches a mailbox only through a
    // worker's own intake, so a target with no worker has nothing to enqueue, and
    // the successor that does arrive overwrites the registration before its first
    // entry. Only a registration ever displaces a registration, which is what
    // makes the ordering above unable to strand anyone.
    let entries_held = state
        .entries
        .values()
        .filter(|entry| entry.target == key)
        .count();
    let reap = if entries_held == 0 {
        state.mailboxes.remove(&key);
        TargetReap::Reclaimed
    } else {
        TargetReap::Retained { entries_held }
    };
    // The sequence surviving a reap is the property everything else rests on,
    // and it is invisible from outside the relay until the next claim exposes
    // it. Reported here, under the same lock as the reclamation, so the figure
    // describes the state the reap left rather than a later reading of it.
    let issued = state
        .generations
        .get(&key)
        .and_then(|held| held.issued)
        .map(ConsumerGenerationId::value);
    emit_inscription(
        INSCRIPTION_MAILBOX_REAPED,
        &json!({
            "namespace": target.namespace,
            "target_session": target.target_session,
            "released_generation": outgoing.map(ConsumerGenerationId::value),
            "highest_generation_issued": issued,
            "mailbox_reclaimed": reap == TargetReap::Reclaimed,
            "entries_held": entries_held,
        }),
    );
    Ok(reap)
}

/// A reap gives up only what it names, takes the mailbox with it, and never
/// takes the sequence.
///
/// Inline for the reason given on the peek block: the ledger is a process-global
/// behind a crate-private lock, and widening these operations to reach them from
/// `tests/` would publish the delivery ledger itself.
///
/// One test because the three are one property rather than three. A reap that
/// reclaims without naming steals a live consumer's mailbox; one that names
/// without reclaiming leaves the record it exists to reclaim; and one that does
/// both but takes the sequence along re-opens the collision the sequence exists
/// to prevent, at exactly the moment a target is recreated. Each is the failure
/// the other two would not catch.
#[cfg(test)]
mod mailbox_reap_tests {
    use crate::protocol::operations::PeekRejection;

    use super::super::super::terminal::terminalize;
    use super::super::fixtures::{claim, mail, peeked, place, request, target, target_generation};
    use super::super::generation::claim_consumer_generation;
    use super::super::peek::peek;
    use super::*;

    #[test]
    fn a_reap_gives_up_what_it_names_and_leaves_the_sequence_behind() {
        // A target nobody claimed is reaped by a caller naming nothing, which is
        // the shape every reap has today: no worker claims a generation until
        // the delivery-loop executors arrive.
        let unclaimed = "mbx-reap-unclaimed";
        place(unclaimed, "mbx-reap-unclaimed-1", 1, mail("body"));
        terminalize("mbx-reap-unclaimed-1");
        assert_eq!(
            reap_target(&target(unclaimed), None),
            Ok(TargetReap::Reclaimed),
            "a target nobody holds is reaped by a caller that holds nothing"
        );

        let namespace = "mbx-reap";
        let held = claim(namespace);
        for index in 1..=2 {
            place(namespace, &format!("{namespace}-{index}"), 1, mail("body"));
        }

        // Naming nothing against a claimed target is a mismatch rather than a
        // wildcard. This is the ordering the naming exists for, read from the
        // other side: a worker that never claimed must not reap a target that
        // somebody else has since claimed.
        assert_eq!(
            reap_target(&target(namespace), None),
            Err(GenerationRejection::NotActive {
                active: Some(held.generation)
            }),
            "a caller holding no generation reaps nothing from a target that is held"
        );
        assert_eq!(
            reap_target(&target(namespace), Some(stale_of(&held))),
            Err(GenerationRejection::NotActive {
                active: Some(held.generation)
            }),
            "and neither does one naming a generation the target does not hold"
        );
        // The refusals left the incumbent in place rather than merely reporting.
        // Asserting the return value alone would pass against a reap that
        // cleared the owner and said no.
        assert_eq!(
            claim_consumer_generation(&target(namespace)),
            Err(GenerationRejection::AlreadyHeld {
                active: held.generation
            }),
            "a refused reap leaves the target held"
        );
        assert_eq!(
            peeked(&held, 10, 1_000),
            vec![1, 2],
            "and leaves its mailbox where it was"
        );

        // Entries still held. The generation is given up — nothing is consuming
        // this target any more — but the mailbox stays, because the reap knows
        // these entries by id and could not report one of them if it resolved
        // them. Dropping them here would be the silent loss the retention is
        // there to refuse.
        assert_eq!(
            reap_target(&target(namespace), Some(held.generation)),
            Ok(TargetReap::Retained { entries_held: 2 }),
            "a reap with entries outstanding gives up the target but keeps its mailbox"
        );
        let inherited = claim_consumer_generation(&target(namespace))
            .expect("the reaped target is free to claim");
        assert_eq!(
            peeked(
                &crate::protocol::identity::ConsumerBinding::new(target(namespace), inherited),
                10,
                1_000
            ),
            vec![1, 2],
            "the retained entries are still there for whoever claims the target next"
        );

        // Now empty. The whole mailbox goes: not only its slots but the cursor
        // and the next position it would hand out, which is what the sequence
        // restarting from one below actually demonstrates.
        terminalize(&format!("{namespace}-1"));
        terminalize(&format!("{namespace}-2"));
        assert_eq!(
            reap_target(&target(namespace), Some(inherited)),
            Ok(TargetReap::Reclaimed),
            "a reap with nothing left takes the mailbox with the registration"
        );
        assert_eq!(
            peek(&request(&held, 10, 1_000)).unwrap_err(),
            PeekRejection::UnknownTarget,
            "a reclaimed target is one the relay no longer holds a mailbox for"
        );
        // The discriminating part is the number, not the emptiness. Had the
        // cursor and the next position survived the reclamation, this entry
        // would be position three behind a cursor at two — a mailbox that peeks
        // exactly the same way and is numbered from a target that no longer
        // exists.
        place(namespace, &format!("{namespace}-3"), 1, mail("body"));
        assert_eq!(
            peeked(&claim(namespace), 10, 1_000),
            vec![1],
            "the recreated mailbox numbers from its own start rather than the reaped cursor"
        );

        // And the one thing that did not go. Every claim above drew from this
        // target's sequence, so a sequence reclaimed with the mailbox would hand
        // out an identifier already issued — precisely the collision that makes
        // a stale identifier dangerous rather than merely wrong.
        assert!(
            target_generation(namespace) > inherited.value(),
            "the sequence continued across both reaps rather than restarting"
        );

        // The second-pass shape, which is not hypothetical: a worker whose
        // transport could not be built unregisters BEFORE draining its queue,
        // because the registry lock is what stops a send landing in a receiver
        // nothing will poll again. Its reap therefore arrives while those tasks
        // are still admitted and can only retain. The reclamation is retried
        // once they are resolved, and it names nothing — the first pass already
        // gave the generation up, so naming the generation it held would be
        // refused by the target it just released.
        let deferred = "mbx-reap-deferred";
        let owner = claim(deferred);
        place(deferred, "mbx-reap-deferred-1", 1, mail("body"));
        assert_eq!(
            reap_target(&target(deferred), Some(owner.generation)),
            Ok(TargetReap::Retained { entries_held: 1 }),
            "the early reap finds the queue still admitted and keeps the mailbox"
        );
        terminalize("mbx-reap-deferred-1");
        assert_eq!(
            reap_target(&target(deferred), None),
            Ok(TargetReap::Reclaimed),
            "and the retry after the drain names nothing, because nothing holds the target"
        );
    }

    /// A generation this target has not issued, for naming one it does not hold.
    fn stale_of(binding: &crate::protocol::identity::ConsumerBinding) -> ConsumerGenerationId {
        ConsumerGenerationId::new(binding.generation.value() + 7)
    }
}
