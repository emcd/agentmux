//! Unit coverage for the delivery protocol boundary's mailbox vocabulary.
//!
//! The types under test carry the two arithmetic claims the pull model's rules
//! are written against: that a declaration begins at *the cursor plus one*, and
//! that a range named "1 through 5" is five entries. Both are stated once, here,
//! precisely so that no relay or transport call site restates them — which makes
//! them worth pinning even though each is a line of arithmetic.
//!
//! Driven entirely through the public `agentmux::protocol` surface, which is the
//! same surface both delivery call directions use.

use agentmux::envelope::AddressIdentity;
use agentmux::protocol::{
    CursorPosition, DeliveryDoorbell, DeliveryEnvelope, DeliveryMessage, EntryRange, EntrySequence,
    MailboxEntry, MailboxEntryKind, MailboxPayload,
};

fn sequence(value: u64) -> EntrySequence {
    EntrySequence::new(value).expect("a nonzero position is a valid sequence")
}

fn range(from: u64, through: u64) -> EntryRange {
    EntryRange::new(sequence(from), sequence(through)).expect("an ascending range is valid")
}

fn identity(name: &str) -> AddressIdentity {
    AddressIdentity {
        session_name: name.to_string(),
        display_name: None,
    }
}

fn mail_payload() -> MailboxPayload {
    MailboxPayload::Mail(std::sync::Arc::new(DeliveryEnvelope {
        message_id: "m-1".to_string(),
        message: DeliveryMessage {
            body: "ship it".to_string(),
            created_at: "2026-08-29T12:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: identity("alice@party"),
            target: identity("bob@party"),
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: false,
    }))
}

fn raw_payload() -> MailboxPayload {
    MailboxPayload::Raw {
        content: "/status".to_string(),
        append_enter: true,
    }
}

/// A fresh mailbox has acknowledged nothing, and the first declaration against it
/// begins at position one. The cursor and the position are separate types so this
/// mapping exists in exactly one place; if it were an integer increment at each
/// call site, this is the case each of them would get wrong independently.
#[test]
fn a_fresh_cursor_points_at_the_first_position() {
    assert_eq!(CursorPosition::start().value(), 0);
    assert_eq!(
        CursorPosition::start().next_sequence(),
        EntrySequence::first()
    );
    assert_eq!(EntrySequence::first().value(), 1);
}

/// Acknowledging through a position leaves the next declaration beginning at the
/// position after it -- never at the acknowledged one, which would re-serve an
/// entry that already terminalized.
#[test]
fn the_cursor_advances_past_the_acknowledged_position() {
    let cursor = CursorPosition::advanced_through(sequence(5));

    assert_eq!(cursor.value(), 5);
    assert_eq!(cursor.next_sequence(), sequence(6));
}

/// Zero is the cursor's "nothing acknowledged" value, so it is not a position.
/// Admitting it would let a caller name an entry that cannot exist and have the
/// range arithmetic accept it.
#[test]
fn zero_is_not_a_mailbox_position() {
    assert!(EntrySequence::new(0).is_none());
    assert_eq!(EntrySequence::new(1), Some(EntrySequence::first()));
}

/// "Entries 1 through 5" is five entries. The requirements are written in
/// inclusive language, so an exclusive count here would put an off-by-one between
/// the rule and the code enforcing it.
#[test]
fn a_range_counts_both_of_its_ends() {
    assert_eq!(range(1, 5).entries_count(), 5);
    assert_eq!(range(3, 3).entries_count(), 1);
    assert_eq!(range(6, 10).entries_count(), 5);
}

#[test]
fn a_range_contains_both_of_its_ends_and_nothing_outside_them() {
    let declared = range(2, 4);

    assert!(declared.contains(sequence(2)));
    assert!(declared.contains(sequence(3)));
    assert!(declared.contains(sequence(4)));
    assert!(!declared.contains(sequence(1)));
    assert!(!declared.contains(sequence(5)));
}

#[test]
fn a_range_enumerates_exactly_the_positions_it_names() {
    let enumerated: Vec<u64> = range(2, 5).sequences().map(EntrySequence::value).collect();

    assert_eq!(enumerated, vec![2, 3, 4, 5]);
}

/// An inverted range is not a range. Rejecting it at construction is what lets
/// every later rule assume `from <= through` rather than re-check it.
#[test]
fn a_range_whose_end_precedes_its_start_is_refused() {
    assert!(EntryRange::new(sequence(5), sequence(4)).is_none());
    assert!(EntryRange::new(sequence(4), sequence(4)).is_some());
}

/// Raw input is a barrier and mail is not. The distinction is what a peek applies
/// to decide whether an entry joins the run it is accumulating or ends it, and it
/// is read off the payload rather than tracked beside it.
#[test]
fn raw_input_is_a_barrier_and_mail_is_not() {
    assert_eq!(mail_payload().kind(), MailboxEntryKind::Mail);
    assert!(!mail_payload().is_barrier());

    assert_eq!(raw_payload().kind(), MailboxEntryKind::Raw);
    assert!(raw_payload().is_barrier());
}

/// An entry reports its own kind, so a peek never has to match the payload to
/// find out what it is holding.
#[test]
fn an_entry_reports_the_kind_of_its_payload() {
    let entry = MailboxEntry {
        sequence: EntrySequence::first(),
        message_id: "m-1".to_string(),
        canonical_bytes: 42,
        payload: raw_payload(),
    };

    assert_eq!(entry.kind(), MailboxEntryKind::Raw);
    assert!(entry.is_barrier());
}

/// A ring that lands before anyone is waiting is retained rather than dropped.
///
/// This is the one doorbell property worth pinning: correctness never depends on
/// a ring arriving, but an entry admitted moments before an executor begins
/// waiting should not have to sit until the poll backstop, and that is a property
/// of the primitive rather than of the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ring_before_anyone_waits_is_retained() {
    let doorbell = DeliveryDoorbell::new();

    doorbell.ring();

    tokio::time::timeout(std::time::Duration::from_secs(5), doorbell.rung())
        .await
        .expect("a ring delivered before the wait began should still wake it");
}

/// Both handles name the same doorbell: the relay holds one to ring, the target's
/// delivery executor holds the other to wait on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cloned_handle_observes_the_same_doorbell() {
    let relay_side = DeliveryDoorbell::new();
    let executor_side = relay_side.clone();

    let waiting = tokio::spawn(async move { executor_side.rung().await });
    // Retained if the spawned task has not reached its wait yet, so this does not
    // depend on the two tasks interleaving in any particular order.
    relay_side.ring();

    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("a clone should observe a ring made through its sibling")
        .expect("the waiting task should not panic");
}
