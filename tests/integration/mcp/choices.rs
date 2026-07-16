use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

fn decisions_list_call() -> Map<String, Value> {
    Map::from_iter([(
        "command".to_string(),
        Value::String("decisions".to_string()),
    )])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_decisions_returns_pending_requests_from_relay() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_list") => json!({
                    "kind": "choices_list",
                    "schema_version": "1",
                    "pending_requests": [
                        {
                            "message_id": "msg-1",
                            "choice_request_id": "choice-1",
                            "target_session": "bravo",
                            "requested_kind": "execute",
                            "requested_details": {
                                "options": [
                                    {"option_id": "allow_once", "name": "Allow once"},
                                    {"option_id": "reject_once", "name": "Reject once"},
                                ],
                            },
                            "enqueued_at": "2026-05-14T20:00:00Z",
                        },
                        {
                            "message_id": "msg-2",
                            "choice_request_id": "choice-2",
                            "target_session": "charlie",
                            "requested_kind": "read",
                            "requested_details": {
                                "options": [
                                    {"option_id": "allow_always", "name": "Allow"},
                                ],
                            },
                            "enqueued_at": "2026-05-14T20:00:05Z",
                        },
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

    let response = harness.call_tool(2, "list", decisions_list_call()).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    let entries = payload["pending_requests"]
        .as_array()
        .expect("pending_requests array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["choice_request_id"], "choice-1");
    assert_eq!(entries[0]["target_session"], "bravo");
    assert_eq!(entries[0]["requested_kind"], "execute");
    assert_eq!(
        entries[0]["requested_details"]["options"]
            .as_array()
            .expect("options array")
            .len(),
        2
    );
    assert_eq!(entries[1]["choice_request_id"], "choice-2");

    let relay_requests = relay.requests_for_operation("choices_list");
    assert_eq!(relay_requests.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_forwards_decision_and_returns_relay_response() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_pick") => json!({
                    "kind": "choices_pick",
                    "schema_version": "1",
                    "status": "resolved",
                    "choice_request_id": request
                        .get("choice_request_id")
                        .cloned()
                        .unwrap_or(Value::Null),
                    "outcome": request.get("outcome").cloned().unwrap_or(Value::Null),
                    "decided_by": "operator@agentmux",
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

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        ("outcome".to_string(), Value::String("selected".to_string())),
        (
            "option_id".to_string(),
            Value::String("allow_once".to_string()),
        ),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["status"], "resolved");
    assert_eq!(payload["choice_request_id"], "choice-1");
    assert_eq!(payload["outcome"], "selected");
    assert_eq!(payload["decided_by"], "operator@agentmux");
    // reason_code and reason are absent on the wire, so they are omitted rather
    // than serialized as null.
    assert!(payload.get("reason_code").is_none());
    assert!(payload.get("reason").is_none());

    let relay_requests = relay.requests_for_operation("choices_pick");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["choice_request_id"], "choice-1");
    assert_eq!(relay_requests[0]["outcome"], "selected");
    assert_eq!(relay_requests[0]["option_id"], "allow_once");
    assert!(
        relay_requests[0].get("ui_session_id").is_none()
            || relay_requests[0]["ui_session_id"].is_null()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_supports_cancelled_outcome_without_option_id() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_pick") => json!({
                    "kind": "choices_pick",
                    "schema_version": "1",
                    "status": "resolved",
                    "choice_request_id": request
                        .get("choice_request_id")
                        .cloned()
                        .unwrap_or(Value::Null),
                    "outcome": "cancelled",
                    "reason_code": "runtime_choices_request_cancelled",
                    "reason": "choice request was cancelled by UI decision",
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

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-2".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["outcome"], "cancelled");
    assert_eq!(payload["reason_code"], "runtime_choices_request_cancelled");

    let relay_requests = relay.requests_for_operation("choices_pick");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["outcome"], "cancelled");
    assert!(
        relay_requests[0]["option_id"].is_null(),
        "option_id must be omitted on cancelled outcome: {:?}",
        relay_requests[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_rejects_decider_identity_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive choices_pick for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
        ("decided_by".to_string(), Value::String("spoof".to_string())),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_unknown_field_error(&response, &["decided_by"]);
    assert!(relay.requests_for_operation("choices_pick").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_rejects_unknown_fields() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive a request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
        ("stowaway".to_string(), Value::String("value".to_string())),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_unknown_field_error(&response, &["stowaway"]);
    assert!(relay.requests_for_operation("choices_pick").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_rejects_selected_without_option_id() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive choices_pick for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        ("outcome".to_string(), Value::String("selected".to_string())),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("choices_pick").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_rejects_cancelled_with_option_id() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive choices_pick for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
        (
            "option_id".to_string(),
            Value::String("allow_once".to_string()),
        ),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("choices_pick").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_rejects_unknown_command_selector() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive a request for invalid command")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([("command".to_string(), Value::String("garbage".to_string()))]);
    let response = harness.call_tool(2, "list", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("choices_list").is_empty());
    assert!(relay.requests_for_operation("choices_pick").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_decisions_rejects_unknown_arg_fields() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive choices_list for invalid args")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::from_iter([
        (
            "command".to_string(),
            Value::String("decisions".to_string()),
        ),
        (
            "args".to_string(),
            json!({
                "stowaway_field": "value",
            }),
        ),
    ]);
    let response = harness.call_tool(2, "list", arguments).await;

    assert_unknown_field_error(&response, &["args.stowaway_field"]);
    assert!(relay.requests_for_operation("choices_list").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_decisions_preserves_authorization_forbidden_choose_capability() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_list") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "choose",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "choose policy scope does not allow choice polling",
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

    let response = harness.call_tool(2, "list", decisions_list_call()).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(response["error"]["data"]["details"]["capability"], "choose");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_preserves_already_resolved_code_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_pick") => json!({
                    "kind": "error",
                    "error": {
                        "code": "runtime_choices_request_already_resolved",
                        "message": "choice request is already resolved",
                        "details": {
                            "choice_request_id": "choice-1",
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

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("runtime_choices_request_already_resolved")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_preserves_authorization_forbidden_choose_capability() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("choices_pick") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "choose",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "choose policy scope does not allow choice decisions",
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

    let arguments = Map::from_iter([
        (
            "choice_request_id".to_string(),
            Value::String("choice-1".to_string()),
        ),
        (
            "outcome".to_string(),
            Value::String("cancelled".to_string()),
        ),
    ]);
    let response = harness.call_tool(2, "choose", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(response["error"]["data"]["details"]["capability"], "choose");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choose_advertised_in_tool_inventory() {
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
        .expect("tool list array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    assert!(
        names.contains(&"choose"),
        "choose tool advertised: {names:?}"
    );
}
