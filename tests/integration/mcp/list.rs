use std::{collections::BTreeSet, sync::Arc};

use serde_json::{Map, Value, json};

use super::helpers::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_catalog_contains_list_send_and_look() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": BUNDLE_NAME,
                        "state": "up",
                        "sessions": [],
                    },
                }),
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": SENDER_SESSION,
                    "delivery_mode": "sync",
                    "status": "success",
                    "results": [],
                }),
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "requester_session": SENDER_SESSION,
                    "target_session": "bravo",
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_lines": ["line-1"],
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
    let response = harness.list_tools(2).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools list array");
    let names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["list".to_string(), "look".to_string(), "send".to_string()])
    );

    assert!(relay.requests_for_operation("list").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_recipient_payload_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": BUNDLE_NAME,
                        "state": "up",
                        "state_reason_code": null,
                        "state_reason": null,
                        "sessions": [
                            {"id": "bravo", "name": "Bravo", "transport": "tmux"},
                            {"id": "charlie", "transport": "tmux"},
                        ],
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
    let response = harness.call_tool(2, "list", Map::new()).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle_name"], BUNDLE_NAME);
    assert_eq!(payload["state"], "up");
    assert_eq!(payload["recipients"][0]["session_name"], "bravo");
    assert_eq!(payload["recipients"][0]["display_name"], "Bravo");
    assert_eq!(payload["recipients"][1]["session_name"], "charlie");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_reports_relay_unavailable_when_relay_is_not_running() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "list", Map::new()).await;

    assert_eq!(error_code(&response), Some("relay_unavailable"));
    assert_eq!(
        response["error"]["data"]["details"]["relay_socket"],
        Value::String(runtime.relay_socket.display().to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_maps_authorization_forbidden_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "list.read",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "reason": "list visibility denied by policy",
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
    let response = harness.call_tool(2, "list", Map::new()).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(
        response["error"]["data"]["details"]["capability"],
        "list.read"
    );
}
