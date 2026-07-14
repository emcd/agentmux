//! Wire-shape invariants for the `RelayResponse` contract, pinned by direct
//! construction and serialization (no relay boot) to complement the
//! dispatch-driven tests in the sibling clusters.

use agentmux::relay::RelayResponse;

fn send_response(request_id: Option<String>) -> RelayResponse {
    RelayResponse::Send {
        schema_version: "1".to_string(),
        request_id,
        requester_session: "backend@agentmux".to_string(),
        sender_display_name: None,
        authenticated_identity: None,
        on_behalf_of: None,
        results: Vec::new(),
    }
}

/// A `Send` response with no caller-supplied `request_id` must OMIT the field
/// on the wire rather than emit `request_id: null`, matching the
/// absent-optional shape that `Raww` already uses for its `request_id` and
/// `message_id`. See issues/relay/50.
#[test]
fn send_response_omits_absent_request_id() {
    let value = serde_json::to_value(send_response(None)).expect("serialize send response");
    let object = value
        .as_object()
        .expect("send response serializes to an object");
    assert!(
        !object.contains_key("request_id"),
        "absent request_id must be omitted, not serialized as null: {value}"
    );
}

/// A caller-supplied `request_id` is echoed on the wire.
#[test]
fn send_response_echoes_present_request_id() {
    let value = send_response(Some("req-1".to_string()));
    let value = serde_json::to_value(value).expect("serialize send response");
    assert_eq!(value["request_id"], serde_json::json!("req-1"));
}
