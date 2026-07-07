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
                        "message": "requester_session is not in bundle configuration",
                        "details": {"requester_session": SENDER_SESSION},
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
async fn send_surfaces_authenticated_identity_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "authenticated_identity": "alpha@agentmux",
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
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["authenticated_identity"], "alpha@agentmux");
    // `on_behalf_of` is reserved and absent on the wire, so it is omitted
    // entirely rather than surfaced as null.
    assert!(payload.get("on_behalf_of").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_omits_sender_attribution_when_relay_omits_it() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
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
    let payload = decode_tool_payload(&response);

    assert!(payload.get("authenticated_identity").is_none());
    assert!(payload.get("on_behalf_of").is_none());
    // The caller supplied no request_id and the relay omitted
    // sender_display_name, so both are absent rather than serialized as null.
    assert!(payload.get("request_id").is_none());
    assert!(payload.get("sender_display_name").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_includes_request_id_and_display_name_when_relay_provides_them() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "sender_display_name": "Alpha",
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
        "request_id".to_string(),
        Value::String("req-send-1".to_string()),
    );
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);

    // Present optional fields still pass through when the relay populates them.
    assert_eq!(payload["request_id"], "req-send-1");
    assert_eq!(payload["sender_display_name"], "Alpha");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_qualifies_bare_target_with_bound_bundle() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
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

    // The relay rejects bare targets, so the bundle-bound MCP server fills in its
    // bound bundle (`party`) before dispatch.
    let relay_requests = relay.requests_for_operation("send");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(
        relay_requests[0]["targets"],
        json!([format!("bravo@{BUNDLE_NAME}")])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_preserves_already_qualified_target() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
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
        Value::Array(vec![
            Value::String("charlie@peer-bundle".to_string()),
            Value::String("ops@GLOBAL".to_string()),
        ]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    decode_tool_payload(&response);

    // A target that already names its namespace is forwarded verbatim; the client
    // never re-qualifies an explicit `@namespace`.
    let relay_requests = relay.requests_for_operation("send");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(
        relay_requests[0]["targets"],
        json!(["charlie@peer-bundle", "ops@GLOBAL"])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_omits_wire_envelope_namespace_for_suffix_routing() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
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

    // Send is suffix-routed: the relay derives the routing bundle from each
    // target's `@<bundle>` suffix, so the client omits the wire namespace.
    let envelopes = relay.envelopes_for_operation("send");
    assert_eq!(envelopes.len(), 1);
    assert!(
        envelopes[0].get("namespace").is_none(),
        "send must not carry a wire-envelope namespace: {:?}",
        envelopes[0]
    );
}
