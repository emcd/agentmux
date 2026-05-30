use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_up_returns_bundle_transition_from_relay() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("up") => json!({
                    "kind": "bundle_transition",
                    "schema_version": "1",
                    "action": "up",
                    "bundles": [
                        {
                            "bundle_name": BUNDLE_NAME,
                            "outcome": "hosted",
                            "reason_code": null,
                            "reason": null,
                        }
                    ],
                    "changed_bundle_count": 1,
                    "skipped_bundle_count": 0,
                    "failed_bundle_count": 0,
                    "changed_any": true,
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
    arguments.insert("command".to_string(), Value::String("up".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["action"], "up");
    assert_eq!(payload["changed_bundle_count"], 1);
    assert_eq!(payload["skipped_bundle_count"], 0);
    assert_eq!(payload["failed_bundle_count"], 0);
    assert_eq!(payload["changed_any"], true);
    let bundles = payload["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["bundle_name"], BUNDLE_NAME);
    assert_eq!(bundles[0]["outcome"], "hosted");

    let relay_requests = relay.requests_for_operation("up");
    assert_eq!(relay_requests.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_down_returns_bundle_transition_from_relay() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("down") => json!({
                    "kind": "bundle_transition",
                    "schema_version": "1",
                    "action": "down",
                    "bundles": [
                        {
                            "bundle_name": BUNDLE_NAME,
                            "outcome": "unhosted",
                            "reason_code": null,
                            "reason": null,
                        }
                    ],
                    "changed_bundle_count": 1,
                    "skipped_bundle_count": 0,
                    "failed_bundle_count": 0,
                    "changed_any": true,
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
    arguments.insert("command".to_string(), Value::String("down".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["action"], "down");
    assert_eq!(payload["changed_any"], true);
    let bundles = payload["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles[0]["outcome"], "unhosted");

    let relay_requests = relay.requests_for_operation("down");
    assert_eq!(relay_requests.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_up_forwards_skipped_outcome_unchanged() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("up") => json!({
                    "kind": "bundle_transition",
                    "schema_version": "1",
                    "action": "up",
                    "bundles": [
                        {
                            "bundle_name": BUNDLE_NAME,
                            "outcome": "skipped",
                            "reason_code": "already_hosted",
                            "reason": "bundle runtime is already hosted",
                        }
                    ],
                    "changed_bundle_count": 0,
                    "skipped_bundle_count": 1,
                    "failed_bundle_count": 0,
                    "changed_any": false,
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
    arguments.insert("command".to_string(), Value::String("up".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["changed_any"], false);
    assert_eq!(payload["skipped_bundle_count"], 1);
    let bundles = payload["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles[0]["outcome"], "skipped");
    assert_eq!(bundles[0]["reason_code"], "already_hosted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_preserves_authorization_forbidden_updown_capability() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("up") | Some("down") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "updown",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "updown policy scope does not allow bundle up/down transitions",
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
    arguments.insert("command".to_string(), Value::String("up".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(response["error"]["data"]["details"]["capability"], "updown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_rejects_unknown_top_level_fields() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive a request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("up".to_string()));
    arguments.insert("stowaway".to_string(), Value::String("value".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;

    assert_unknown_field_error(&response, &["stowaway"]);
    assert!(relay.requests_for_operation("up").is_empty());
    assert!(relay.requests_for_operation("down").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_rejects_unknown_arg_fields() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive updown for invalid args")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("up".to_string()));
    arguments.insert(
        "args".to_string(),
        json!({
            "stowaway_field": "value",
        }),
    );
    let response = harness.call_tool(2, "updown", arguments).await;

    assert_unknown_field_error(&response, &["args.stowaway_field"]);
    assert!(relay.requests_for_operation("up").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_rejects_unknown_command_selector() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive a request for invalid command")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("garbage".to_string()));
    let response = harness.call_tool(2, "updown", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("up").is_empty());
    assert!(relay.requests_for_operation("down").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_rejects_missing_command_selector() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive a request without command")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = Map::new();
    let response = harness.call_tool(2, "updown", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("up").is_empty());
    assert!(relay.requests_for_operation("down").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updown_advertised_in_tool_inventory() {
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
        names.contains(&"updown"),
        "updown tool advertised: {names:?}"
    );
}
