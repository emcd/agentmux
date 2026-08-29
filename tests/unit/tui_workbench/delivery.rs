//! Stream-event reconciliation for outgoing deliveries: incoming-message
//! dedupe, and how a `delivery_outcome` settles a pending message id
//! regardless of which side of the acknowledgement it arrives on.

use agentmux::relay::{SendOutcome, SendResult};
use serde_json::json;

use super::{UI_ADDRESS, make_state, stream_event};

/// A queued `SendResult`, as the relay returns for an accepted async send.
fn queued_send(target_session: &str, message_id: &str) -> SendResult {
    SendResult {
        target_session: target_session.to_string(),
        message_id: message_id.to_string(),
        outcome: SendOutcome::Queued,
        reason_code: None,
        reason: None,
        details: None,
    }
}

#[test]
fn incoming_message_ids_are_deduplicated() {
    let mut state = make_state();
    let event = stream_event(
        "incoming_message",
        UI_ADDRESS,
        json!({ "message_id": "msg-1", "sender_session": "relay", "body": "hello" }),
    );
    state.record_stream_events(&[event.clone(), event]);

    assert_eq!(state.chat_history_bodies(), vec!["hello"]);
    assert_eq!(state.event_history_len(), 1);
}

#[test]
fn terminal_delivery_outcome_clears_pending_message() {
    let mut state = make_state();
    state.record_chat_events(&[queued_send("user", "msg-1")]);
    assert_eq!(state.pending_deliveries_count(), 1);

    // A `delivery_outcome` envelope targets the delivery recipient (`user`), not
    // this UI; reconciliation keys off the payload `message_id`.
    state.record_stream_events(&[stream_event(
        "delivery_outcome",
        "user@agentmux",
        json!({ "message_id": "msg-1", "phase": "delivered", "outcome": "success" }),
    )]);
    assert_eq!(state.pending_deliveries_count(), 0);
}

#[test]
fn queued_result_does_not_readd_pending_after_terminal_outcome_first() {
    let mut state = make_state();
    state.record_stream_events(&[stream_event(
        "delivery_outcome",
        "user@agentmux",
        json!({ "message_id": "msg-1", "phase": "delivered", "outcome": "success" }),
    )]);
    assert_eq!(state.pending_deliveries_count(), 0);

    state.record_chat_events(&[queued_send("user", "msg-1")]);
    assert_eq!(state.pending_deliveries_count(), 0);
}

/// The evidence-bearing terminal outcomes have to win the same race as
/// `success`. The ordering is the whole point: a terminal event arriving *after*
/// the acknowledgement clears the pending id regardless of its outcome spelling,
/// so an event-after-pending test passes whether or not the outcome joins the
/// terminal set. Only the event-first ordering discriminates, because the
/// acknowledgement consults that set to decide whether to re-add the id.
#[test]
fn queued_result_does_not_readd_pending_after_not_submitted_arrives_first() {
    let mut state = make_state();
    state.record_stream_events(&[stream_event(
        "delivery_outcome",
        "user@agentmux",
        json!({
            "message_id": "msg-1",
            "phase": "not_submitted",
            "outcome": "not_submitted",
        }),
    )]);
    assert_eq!(state.pending_deliveries_count(), 0);
    assert!(
        state
            .event_history_entries()
            .iter()
            .any(|line| line.contains("outcome=not_submitted")),
        "the outcome is recorded under its own spelling, not as an unknown \
         placeholder: {:?}",
        state.event_history_entries()
    );

    state.record_chat_events(&[queued_send("user", "msg-1")]);
    assert_eq!(
        state.pending_deliveries_count(),
        0,
        "a terminal not_submitted that raced ahead of the acknowledgement must \
         keep that acknowledgement from re-adding the message as pending"
    );
}

#[test]
fn queued_result_does_not_readd_pending_after_submission_unknown_arrives_first() {
    let mut state = make_state();
    state.record_stream_events(&[stream_event(
        "delivery_outcome",
        "user@agentmux",
        json!({
            "message_id": "msg-1",
            "phase": "submission_unknown",
            "outcome": "submission_unknown",
        }),
    )]);
    assert_eq!(state.pending_deliveries_count(), 0);
    assert!(
        state
            .event_history_entries()
            .iter()
            .any(|line| line.contains("outcome=submission_unknown")),
        "the outcome is recorded under its own spelling, not as an unknown \
         placeholder: {:?}",
        state.event_history_entries()
    );

    state.record_chat_events(&[queued_send("user", "msg-1")]);
    assert_eq!(
        state.pending_deliveries_count(),
        0,
        "a terminal submission_unknown that raced ahead of the acknowledgement \
         must keep that acknowledgement from re-adding the message as pending"
    );
}
