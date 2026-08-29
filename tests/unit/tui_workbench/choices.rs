//! Pending choice requests: the order they are presented in, how a snapshot
//! placeholder hydrates from a later request, what resolution removes, and the
//! filter that scopes the look pane to the active interaction target.

use agentmux::{relay::RelayStreamEvent, runtime::error::RuntimeError};
use serde_json::json;

use super::{UI_ADDRESS, make_state, stream_event};

/// A `choices.requested` event in wire shape: the envelope targets this UI
/// session, while the canonical choice target lives in the payload.
fn choice_requested(
    message_id: &str,
    choice_request_id: &str,
    canonical_target: &str,
    option_ids: &[&str],
) -> RelayStreamEvent {
    let options = option_ids
        .iter()
        .map(|option_id| json!({ "option_id": option_id }))
        .collect::<Vec<_>>();
    stream_event(
        "choices.requested",
        UI_ADDRESS,
        json!({
            "message_id": message_id,
            "choice_request_id": choice_request_id,
            "target_session": canonical_target,
            "requested_kind": "approval",
            "requested_details": { "options": options },
        }),
    )
}

#[test]
fn same_session_pending_choices_order_fifo_by_enqueued_at() {
    let mut state = make_state();
    // Requests arrive for one session out of enqueued_at order (the relay may
    // replay or reorder on the wire); the pending list must present them FIFO by
    // enqueued_at regardless of arrival order.
    state.inject_choice_request("req-c", "acp@agentmux", Some("2026-06-24T00:00:03Z"));
    state.inject_choice_request("req-a", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-b", "acp@agentmux", Some("2026-06-24T00:00:02Z"));

    assert_eq!(
        state.pending_choice_request_ids(),
        vec!["req-a", "req-b", "req-c"],
        "pending choices must be ordered FIFO by enqueued_at"
    );
}

#[test]
fn pending_choices_tie_break_by_request_id_and_sink_missing_enqueued_at() {
    let mut state = make_state();
    // Two requests share an enqueued_at: ties break deterministically by
    // choice_request_id. A request with no enqueued_at sorts last.
    state.inject_choice_request("req-z", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-a", "acp@agentmux", Some("2026-06-24T00:00:01Z"));
    state.inject_choice_request("req-m", "acp@agentmux", None);

    assert_eq!(
        state.pending_choice_request_ids(),
        vec!["req-a", "req-z", "req-m"],
        "equal enqueued_at ties break by choice_request_id; missing enqueued_at sorts last"
    );
}

#[test]
fn choice_snapshot_and_replayed_request_keep_a_single_row() {
    let mut state = make_state();
    state.record_stream_events(&[stream_event(
        "choices.snapshot",
        UI_ADDRESS,
        json!({ "pending_count": 1, "choice_request_ids": ["perm-1"] }),
    )]);
    assert_eq!(state.pending_choice_request_ids(), vec!["perm-1"]);

    // A subsequent request for the same id hydrates the snapshot placeholder in
    // place, and a duplicate delivery of that request must not add a second row.
    let requested = choice_requested(
        "msg-1",
        "perm-1",
        "acp@agentmux",
        &["allow-once", "reject-once"],
    );
    state.record_stream_events(&[requested.clone(), requested]);

    let pending = state.pending_choices();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].choice_request_id, "perm-1");
    assert_eq!(pending[0].message_id.as_deref(), Some("msg-1"));
    assert_eq!(pending[0].target_session.as_deref(), Some("acp@agentmux"));
    assert_eq!(pending[0].requested_kind.as_deref(), Some("approval"));
    assert_eq!(pending[0].option_ids, vec!["allow-once", "reject-once"]);
}

#[test]
fn resolved_choice_removes_the_pending_request() {
    let mut state = make_state();
    state.record_stream_events(&[choice_requested(
        "msg-1",
        "perm-1",
        "acp@agentmux",
        &["allow-once"],
    )]);
    assert_eq!(state.pending_choice_request_ids(), vec!["perm-1"]);

    state.record_stream_events(&[stream_event(
        "choices.resolved",
        UI_ADDRESS,
        json!({ "choice_request_id": "perm-1", "outcome": "selected" }),
    )]);
    assert!(state.pending_choice_request_ids().is_empty());
}

#[test]
fn look_choice_resolution_without_pending_request_is_validation_error() {
    let mut state = make_state();
    state.set_interaction_target("acp");

    match state.resolve_selected_look_choice_selected() {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_choice_request");
        }
        other => panic!("unexpected result: {other:?}"),
    }
    match state.resolve_selected_look_choice_cancelled() {
        Err(RuntimeError::Validation { code, .. }) => {
            assert_eq!(code, "validation_unknown_choice_request");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn look_pending_choices_are_filtered_to_the_active_target() {
    let mut state = make_state();
    state.record_stream_events(&[choice_requested("msg-1", "perm-1", "acp@agentmux", &[])]);
    state.record_stream_events(&[choice_requested("msg-2", "perm-2", "relay@agentmux", &[])]);
    // The stored choice target is the canonical payload id, so the active
    // interaction target is matched in the same canonical form.
    state.set_interaction_target("acp@agentmux");

    assert_eq!(state.look_pending_choice_request_ids(), vec!["perm-1"]);
}
