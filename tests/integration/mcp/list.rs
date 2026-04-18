use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::{Map, Value, json};

use super::helpers::*;

fn list_sessions_call(args: Map<String, Value>) -> Map<String, Value> {
    Map::from_iter([
        ("command".to_string(), Value::String("sessions".to_string())),
        ("args".to_string(), Value::Object(args)),
    ])
}

fn relay_socket_for(runtime: &TestRuntime, bundle_name: &str) -> PathBuf {
    runtime
        .state_root
        .join("bundles")
        .join(bundle_name)
        .join("relay.sock")
}

fn ensure_socket_parent(socket_path: &Path) {
    let parent = socket_path
        .parent()
        .expect("relay socket must have parent directory");
    fs::create_dir_all(parent).expect("create relay socket parent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_catalog_contains_list_sessions_send_and_look() {
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
        BTreeSet::from([
            "help".to_string(),
            "list".to_string(),
            "look".to_string(),
            "send".to_string(),
        ])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_returns_canonical_bundle_payload_from_relay() {
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
                            {"id": "charlie", "transport": "acp"},
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
    let response = harness
        .call_tool(2, "list", list_sessions_call(Map::new()))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle"]["id"], BUNDLE_NAME);
    assert_eq!(payload["bundle"]["state"], "up");
    assert_eq!(payload["bundle"]["sessions"][0]["id"], "bravo");
    assert_eq!(payload["bundle"]["sessions"][0]["name"], "Bravo");
    assert_eq!(payload["bundle"]["sessions"][1]["id"], "charlie");
    assert_eq!(payload["bundle"]["sessions"][1]["transport"], "acp");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_rejects_conflicting_bundle_and_all_selectors() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let arguments = list_sessions_call(Map::from_iter([
        (
            "bundle_name".to_string(),
            Value::String(BUNDLE_NAME.to_string()),
        ),
        ("all".to_string(), Value::Bool(true)),
    ]));
    let response = harness.call_tool(2, "list", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_rejects_missing_or_invalid_command() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;

    let missing_command = harness.call_tool(2, "list", Map::new()).await;
    assert_eq!(
        error_code(&missing_command),
        Some("validation_invalid_params")
    );

    let invalid_command = harness
        .call_tool(
            2,
            "list",
            Map::from_iter([("command".to_string(), Value::String("bundles".to_string()))]),
        )
        .await;
    assert_eq!(
        error_code(&invalid_command),
        Some("validation_invalid_params")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_rejects_stringified_args_with_informative_validation_error() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(
            2,
            "list",
            Map::from_iter([
                ("command".to_string(), Value::String("sessions".to_string())),
                (
                    "args".to_string(),
                    Value::String("{\"all\":true}".to_string()),
                ),
            ]),
        )
        .await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert_eq!(
        response["error"]["data"]["message"],
        Value::String("invalid args for list command".to_string())
    );
    assert_eq!(
        response["error"]["data"]["details"]["reason"],
        Value::String("args must be a JSON object, got string".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_synthesizes_down_bundle_for_unreachable_home_bundle() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "list", list_sessions_call(Map::new()))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle"]["id"], BUNDLE_NAME);
    assert_eq!(payload["bundle"]["state"], "down");
    assert_eq!(payload["bundle"]["state_reason_code"], "not_started");
    assert_eq!(
        payload["bundle"]["sessions"]
            .as_array()
            .map_or(0, |value| value.len()),
        3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_rejects_unreachable_non_home_bundle_with_relay_unavailable() {
    let runtime = TestRuntime::create();
    write_bundle_configuration(&runtime.config_root, "zeta", &["zeta"]);
    let mut harness = McpHarness::spawn(&runtime).await;
    let arguments = list_sessions_call(Map::from_iter([(
        "bundle_name".to_string(),
        Value::String("zeta".to_string()),
    )]));
    let response = harness.call_tool(2, "list", arguments).await;

    assert_eq!(error_code(&response), Some("relay_unavailable"));
    assert_eq!(
        response["error"]["data"]["details"]["relay_socket"],
        Value::String(relay_socket_for(&runtime, "zeta").display().to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_all_mode_aggregates_in_lexicographic_bundle_order() {
    let runtime = TestRuntime::create();
    write_bundle_configuration(&runtime.config_root, "alpha", &["alpha"]);
    write_bundle_configuration(&runtime.config_root, "zeta", &["zeta"]);
    let alpha_socket = relay_socket_for(&runtime, "alpha");
    let zeta_socket = relay_socket_for(&runtime, "zeta");
    ensure_socket_parent(&alpha_socket);
    ensure_socket_parent(&zeta_socket);
    let alpha_relay = FakeRelay::start(
        alpha_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "alpha",
                        "state": "up",
                        "state_reason_code": null,
                        "state_reason": null,
                        "sessions": [{"id": "alpha", "transport": "tmux"}],
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
    let zeta_relay = FakeRelay::start(
        zeta_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "zeta",
                        "state": "up",
                        "state_reason_code": null,
                        "state_reason": null,
                        "sessions": [{"id": "zeta", "transport": "acp"}],
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
    let arguments = list_sessions_call(Map::from_iter([("all".to_string(), Value::Bool(true))]));
    let response = harness.call_tool(2, "list", arguments).await;
    let payload = decode_tool_payload(&response);
    let bundles = payload["bundles"]
        .as_array()
        .expect("bundles must be array");
    let bundle_ids = bundles
        .iter()
        .filter_map(|bundle| bundle["id"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(bundle_ids, vec!["alpha", "party", "zeta"]);
    assert_eq!(bundles[1]["state"], "down");
    assert_eq!(bundles[1]["state_reason_code"], "not_started");
    assert_eq!(alpha_relay.requests_for_operation("list").len(), 1);
    assert_eq!(zeta_relay.requests_for_operation("list").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_all_mode_fails_fast_on_first_authorization_denial() {
    let runtime = TestRuntime::create();
    write_bundle_configuration(&runtime.config_root, "alpha", &["alpha"]);
    write_bundle_configuration(&runtime.config_root, "zeta", &["zeta"]);
    let alpha_socket = relay_socket_for(&runtime, "alpha");
    let zeta_socket = relay_socket_for(&runtime, "zeta");
    ensure_socket_parent(&alpha_socket);
    ensure_socket_parent(&zeta_socket);
    let alpha_relay = FakeRelay::start(
        alpha_socket,
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
                            "bundle_name": "alpha",
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
    let party_relay = FakeRelay::start(
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
                        "sessions": [{"id": BUNDLE_NAME, "transport": "tmux"}],
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
    let zeta_relay = FakeRelay::start(
        zeta_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "zeta",
                        "state": "up",
                        "state_reason_code": null,
                        "state_reason": null,
                        "sessions": [{"id": "zeta", "transport": "tmux"}],
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
    let arguments = list_sessions_call(Map::from_iter([("all".to_string(), Value::Bool(true))]));
    let response = harness.call_tool(2, "list", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(alpha_relay.requests_for_operation("list").len(), 1);
    assert_eq!(party_relay.requests_for_operation("list").len(), 0);
    assert_eq!(zeta_relay.requests_for_operation("list").len(), 0);
}
