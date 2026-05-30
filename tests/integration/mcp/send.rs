use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_conflicting_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
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
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("targets".to_string(), Value::Array(Vec::new()));
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_empty_targets"));
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_message_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
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
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_invalid_quiescence_timeout_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
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
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_invalid_acp_turn_timeout_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
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
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_conflicting_timeout_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
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
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_unknown_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive send request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("unexpected".to_string(), Value::Bool(true));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_unknown_field_error(&response, &["unexpected"]);
    assert!(relay.requests_for_operation("send").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_quiescence_timeout_override() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
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
        "quiescence_timeout_ms".to_string(),
        Value::Number(1234.into()),
    );
    let response = harness.call_tool(2, "send", arguments).await;
    decode_tool_payload(&response);

    let relay_requests = relay.requests_for_operation("send");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["quiescence_timeout_ms"], 1234);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_acp_turn_timeout_override() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
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
    arguments.insert("acp_turn_timeout_ms".to_string(), Value::Number(987.into()));
    let response = harness.call_tool(2, "send", arguments).await;
    decode_tool_payload(&response);

    let relay_requests = relay.requests_for_operation("send");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["acp_turn_timeout_ms"], 987);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_maps_unknown_sender_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
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
                Some("send") => json!({
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
                Some("send") => json!({
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_advertises_optional_namespace_in_tool_schema() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| {
            json!({
                "kind": "error",
                "error": {"code": "internal_unexpected_failure", "message": "unused"},
            })
        }),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness.list_tools(2).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools list array");
    let send_tool = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("send"))
        .expect("send tool present");
    let properties = send_tool["inputSchema"]["properties"]
        .as_object()
        .expect("send inputSchema properties object");
    assert!(
        properties.contains_key("namespace"),
        "send tool schema advertises namespace: {properties:?}"
    );
    let required = send_tool["inputSchema"]["required"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(
        !required.iter().any(|field| field == "namespace"),
        "namespace is optional on send: {required:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_namespace_to_wire_envelope() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
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
    arguments.insert("namespace".to_string(), Value::String("GLOBAL".to_string()));
    let response = harness.call_tool(2, "send", arguments).await;
    decode_tool_payload(&response);

    let envelopes = relay.envelopes_for_operation("send");
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["namespace"], "GLOBAL");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_omitting_namespace_falls_back_to_bound_bundle() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
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
    let response = harness.call_tool(2, "send", arguments).await;
    decode_tool_payload(&response);

    let envelopes = relay.envelopes_for_operation("send");
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["namespace"], BUNDLE_NAME);
}
