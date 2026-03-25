use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_conflicting_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(true));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_conflicting_targets")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("targets".to_string(), Value::Array(Vec::new()));
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_empty_targets"));
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_message_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("   ".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_arguments"));
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_invalid_quiescence_timeout_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert("quiescence_timeout_ms".to_string(), Value::Number(0.into()));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_invalid_quiescence_timeout")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_invalid_acp_turn_timeout_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert("acp_turn_timeout_ms".to_string(), Value::Number(0.into()));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_invalid_acp_turn_timeout")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_conflicting_timeout_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert(
        "quiescence_timeout_ms".to_string(),
        Value::Number(1234.into()),
    );
    arguments.insert(
        "acp_turn_timeout_ms".to_string(),
        Value::Number(5678.into()),
    );
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_conflicting_timeout_fields")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_returns_partial_and_forwards_sender_session() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "sender_display_name": "Alpha",
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "partial",
                    "results": [
                        {
                            "target_session": "bravo",
                            "message_id": "msg-1",
                            "outcome": "delivered",
                        },
                        {
                            "target_session": "charlie",
                            "message_id": "msg-2",
                            "outcome": "timeout",
                            "reason": "delivery_quiescence_timeout",
                        }
                    ],
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("request_id".to_string(), Value::String("req-7".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![
            Value::String("bravo".to_string()),
            Value::String("charlie".to_string()),
        ]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["status"], "partial");
    assert_eq!(payload["sender_session"], SENDER_SESSION);
    assert_eq!(payload["sender_display_name"], "Alpha");
    assert_eq!(payload["delivery_mode"], "async");
    assert_eq!(payload["results"][1]["outcome"], "timeout");
    assert_eq!(
        payload["results"][1]["reason"],
        "delivery_quiescence_timeout"
    );

    let relay_requests = relay.requests_for_operation("chat");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["sender_session"], SENDER_SESSION);
    assert_eq!(relay_requests[0]["targets"][0], "bravo");
    assert_eq!(relay_requests[0]["targets"][1], "charlie");
    assert_eq!(relay_requests[0]["delivery_mode"], "async");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_sync_mode_and_timeout_override() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "success",
                    "results": [],
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert(
        "delivery_mode".to_string(),
        Value::String("sync".to_string()),
    );
    arguments.insert(
        "quiescence_timeout_ms".to_string(),
        Value::Number(1234.into()),
    );
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["delivery_mode"], "sync");

    let relay_requests = relay.requests_for_operation("chat");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["delivery_mode"], "sync");
    assert_eq!(relay_requests[0]["quiescence_timeout_ms"], 1234);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_acp_turn_timeout_override() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "success",
                    "results": [],
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert(
        "delivery_mode".to_string(),
        Value::String("sync".to_string()),
    );
    arguments.insert("acp_turn_timeout_ms".to_string(), Value::Number(987.into()));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["delivery_mode"], "sync");

    let relay_requests = relay.requests_for_operation("chat");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["delivery_mode"], "sync");
    assert_eq!(relay_requests[0]["acp_turn_timeout_ms"], 987);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_maps_unknown_sender_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "error",
                    "error": {
                        "code": "validation_unknown_sender",
                        "message": "sender_session is not in bundle configuration",
                        "details": {"sender_session": SENDER_SESSION},
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_unknown_sender"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_maps_authorization_forbidden_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "send.deliver",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "send policy scope does not allow cross-bundle delivery",
                            "targets": ["bravo"],
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(
        response["error"]["data"]["details"]["capability"],
        "send.deliver"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_preserves_reserved_capability_label_from_relay_denial() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "do.run",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "capability currently disallowed",
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
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(response["error"]["data"]["details"]["capability"], "do.run");
}
