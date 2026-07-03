//! Stale vs live prior-writer semantics for `register_stream`'s
//! identity-claim conflict decision. Drives the decision directly so the
//! closed-writer case is deterministic, free of the connection layer's
//! write-timeout teardown race.

use super::*;

// A stale attachment -- a registry entry whose prior connection's writer task has
// exited (closed writer) but whose drop-guard has not yet cleared it -- must not
// block a fresh claim: `register_stream` reclaims the entry and registers rather
// than reporting an identity-claim conflict. Drives the decision directly so the
// closed-writer case is deterministic, free of the connection layer's
// write-timeout teardown race.
#[test]
fn second_claim_against_closed_prior_writer_is_reclaimed_not_conflicted() {
    assert!(
        !second_claim_is_live_conflict_for_testing(false),
        "a closed prior writer is a stale attachment and must be reclaimed"
    );
}

// An open prior writer is a genuine concurrent owner: a second claim for the same
// principal must be rejected as a live identity-claim conflict.
#[test]
fn second_claim_against_open_prior_writer_is_a_live_conflict() {
    assert!(
        second_claim_is_live_conflict_for_testing(true),
        "an open prior writer is a live owner and must conflict"
    );
}
