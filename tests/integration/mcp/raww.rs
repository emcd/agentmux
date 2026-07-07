use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_rejects_sender_like_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive raww request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "requester_session".to_string(),
        Value::String("spoof".to_string()),
    );
    let response = harness.call_tool(2, "raww", arguments).await;

    assert_unknown_field_error(&response, &["requester_session"]);
    assert!(relay.requests_for_operation("raww").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_returns_queued_payload_and_forwards_request_shape() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("raww") => json!({
                    "kind": "raww",
                    "schema_version": "1",
                    "status": "queued",
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "transport": "tmux",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "message_id": "raww-1",
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello".to_string()));
    arguments.insert("no_enter".to_string(), Value::Bool(true));
    arguments.insert(
        "request_id".to_string(),
        Value::String("req-raww-1".to_string()),
    );
    let response = harness.call_tool(2, "raww", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["status"], "queued");
    // Bare `bravo` is qualified to the bound bundle before the relay call, so the
    // relay echoes the qualified target back.
    assert_eq!(payload["target_session"], format!("bravo@{BUNDLE_NAME}"));
    assert_eq!(payload["transport"], "tmux");
    assert_eq!(payload["request_id"], "req-raww-1");
    assert_eq!(payload["message_id"], "raww-1");
    assert!(
        payload.get("details").is_none(),
        "raww response must not carry a details field: {payload:?}"
    );

    let relay_requests = relay.requests_for_operation("raww");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["requester_session"], SENDER_SESSION);
    assert_eq!(
        relay_requests[0]["target_session"],
        format!("bravo@{BUNDLE_NAME}")
    );
    assert_eq!(relay_requests[0]["text"], "hello");
    assert_eq!(relay_requests[0]["no_enter"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_returns_queued_payload_for_acp_target() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("raww") => json!({
                    "kind": "raww",
                    "schema_version": "1",
                    "status": "queued",
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "transport": "acp",
                    "message_id": "raww-acp-1",
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("charlie".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello acp".to_string()));
    let response = harness.call_tool(2, "raww", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["status"], "queued");
    // Bare `charlie` is qualified to the bound bundle before the relay call.
    assert_eq!(payload["target_session"], format!("charlie@{BUNDLE_NAME}"));
    assert_eq!(payload["transport"], "acp");
    // The relay omitted request_id, so it is absent (not null); message_id is
    // present and passes through.
    assert!(payload.get("request_id").is_none());
    assert_eq!(payload["message_id"], "raww-acp-1");
    assert!(
        payload.get("details").is_none(),
        "raww response must not carry a details field: {payload:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_maps_unknown_target_validation_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("raww") => json!({
                    "kind": "error",
                    "error": {
                        "code": "validation_unknown_target",
                        "message": "target_session is not in bundle configuration",
                        "details": {"target_session": "missing"},
                    },
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("missing".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello".to_string()));
    let response = harness.call_tool(2, "raww", arguments).await;

    assert_eq!(error_code(&response), Some("validation_unknown_target"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_maps_authorization_forbidden_and_preserves_capability_label() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("raww") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "raww.write",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "target_session": "bravo",
                            "reason": "raww policy scope does not allow target write",
                        },
                    },
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello".to_string()));
    let response = harness.call_tool(2, "raww", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(
        response["error"]["data"]["details"]["capability"],
        "raww.write"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raww_omits_wire_envelope_namespace_for_suffix_routing() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("raww") => json!({
                    "kind": "raww",
                    "schema_version": "1",
                    "status": "delivered",
                    "target_session": "bravo",
                    "transport": "tmux",
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("text".to_string(), Value::String("hello".to_string()));
    let response = harness.call_tool(2, "raww", arguments).await;
    decode_tool_payload(&response);

    // Raww is suffix-routed (mirrors Send): the bare `bravo` target is qualified to
    // the bound bundle (`bravo@party`) before the relay call, and the relay derives
    // the routing bundle from that `@<bundle>` suffix, so the client omits the wire
    // namespace.
    let envelopes = relay.envelopes_for_operation("raww");
    assert_eq!(envelopes.len(), 1);
    assert!(
        envelopes[0].get("namespace").is_none(),
        "raww must not carry a wire-envelope namespace: {:?}",
        envelopes[0]
    );
}
