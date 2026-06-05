use std::{collections::BTreeSet, collections::HashMap, sync::Arc};

use serde_json::{Map, Value, json};

use super::helpers::*;

fn list_sessions_call(args: Map<String, Value>) -> Map<String, Value> {
    Map::from_iter([
        ("command".to_string(), Value::String("sessions".to_string())),
        ("args".to_string(), Value::Object(args)),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_catalog_contains_list_sessions_send_look_and_raww() {
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
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
                    },
                }),
                Some("send") => json!({
                    "kind": "send",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "requester_session": SENDER_SESSION,
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
            "change".to_string(),
            "grant".to_string(),
            "help".to_string(),
            "list".to_string(),
            "look".to_string(),
            "new".to_string(),
            "raww".to_string(),
            "send".to_string(),
            "updown".to_string(),
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
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [
                            {"id": "bravo", "name": "Bravo", "transport": "tmux", "ready": true},
                            {"id": "charlie", "transport": "acp", "ready": true},
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
    let bundles = payload["bundles"]
        .as_array()
        .expect("bundles must be array");
    assert_eq!(bundles.len(), 2);
    let configured = &bundles[0];
    assert_eq!(configured["id"], BUNDLE_NAME);
    assert_eq!(configured["hosted"], true);
    assert_eq!(configured["state"], "up");
    assert_eq!(configured["startup_health"], "healthy");
    assert_eq!(configured["startup_failure_count"], 0);
    assert!(
        configured["recent_startup_failures"]
            .as_array()
            .is_some_and(|values| values.is_empty())
    );
    assert_eq!(configured["sessions"][0]["id"], "bravo");
    assert_eq!(configured["sessions"][0]["name"], "Bravo");
    assert_eq!(configured["sessions"][0]["ready"], true);
    assert_eq!(configured["sessions"][1]["id"], "charlie");
    assert_eq!(configured["sessions"][1]["transport"], "acp");
    assert_eq!(configured["sessions"][1]["ready"], true);
    assert_eq!(bundles[1]["id"], "GLOBAL");
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
async fn list_rejects_unknown_top_level_fields() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(
            2,
            "list",
            Map::from_iter([
                ("command".to_string(), Value::String("sessions".to_string())),
                ("bogus".to_string(), Value::Bool(true)),
            ]),
        )
        .await;

    assert_unknown_field_error(&response, &["bogus"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_rejects_unknown_arg_fields() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(
            2,
            "list",
            list_sessions_call(Map::from_iter([(
                "stowaway".to_string(),
                Value::String("value".to_string()),
            )])),
        )
        .await;

    assert_unknown_field_error(&response, &["args.stowaway"]);
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
    let bundles = payload["bundles"]
        .as_array()
        .expect("bundles must be array");
    assert_eq!(bundles.len(), 2);
    let configured = &bundles[0];
    assert_eq!(configured["id"], BUNDLE_NAME);
    assert_eq!(configured["hosted"], false);
    assert_eq!(configured["state"], "down");
    assert_eq!(configured["state_reason_code"], "not_started");
    assert_eq!(configured["startup_failure_count"], 0);
    assert!(
        configured["recent_startup_failures"]
            .as_array()
            .is_some_and(|values| values.is_empty())
    );
    let sessions = configured["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 3);
    for session in sessions {
        assert_eq!(session["ready"], false);
    }
    let global = &bundles[1];
    assert_eq!(global["id"], "GLOBAL");
    assert_eq!(global["state"], "down");
    assert_eq!(global["state_reason_code"], "relay_unavailable");
    assert!(
        global["sessions"]
            .as_array()
            .is_some_and(|values| values.is_empty())
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
        Value::String(runtime.relay_socket.display().to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_all_mode_aggregates_in_lexicographic_bundle_order() {
    let runtime = TestRuntime::create();
    write_bundle_configuration(&runtime.config_root, "alpha", &["alpha"]);
    write_bundle_configuration(&runtime.config_root, "zeta", &["zeta"]);
    let mut routes: HashMap<String, RelayResponder> = HashMap::new();
    routes.insert(
        "alpha".to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "alpha",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [{"id": "alpha", "transport": "tmux", "ready": true}],
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
    routes.insert(
        "zeta".to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "zeta",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [{"id": "zeta", "transport": "acp", "ready": true}],
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
    routes.insert(
        "party".to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "party",
                        "hosted": false,
                        "state": "down",
                        "startup_health": null,
                        "state_reason_code": "not_started",
                        "state_reason": "bundle runtime not started",
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
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
    routes.insert("GLOBAL".to_string(), default_empty_global_responder());
    let relay = FakeRelay::start_for_bundles(runtime.relay_socket.clone(), routes);
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

    assert_eq!(bundle_ids, vec!["alpha", "party", "zeta", "GLOBAL"]);
    assert_eq!(bundles[0]["hosted"], true);
    assert_eq!(bundles[1]["hosted"], false);
    assert_eq!(bundles[2]["hosted"], true);
    assert_eq!(bundles[3]["hosted"], false);
    assert_eq!(bundles[0]["sessions"][0]["ready"], true);
    assert_eq!(bundles[2]["sessions"][0]["ready"], true);
    assert_eq!(bundles[1]["state"], "down");
    assert_eq!(bundles[1]["state_reason_code"], "not_started");
    assert_eq!(bundles[1]["startup_failure_count"], 0);
    assert_eq!(bundles[3]["state"], "down");
    assert!(
        bundles[3]["sessions"]
            .as_array()
            .is_some_and(|values| values.is_empty())
    );
    assert_eq!(relay.requests_for_operation("list").len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_appends_global_bundle_with_relay_wide_principals() {
    let runtime = TestRuntime::create();
    let mut routes: HashMap<String, RelayResponder> = HashMap::new();
    routes.insert(
        BUNDLE_NAME.to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": BUNDLE_NAME,
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [],
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
    routes.insert(
        "GLOBAL".to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "GLOBAL",
                        "hosted": true,
                        "state": "up",
                        "startup_health": null,
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [
                            {"id": "operator@GLOBAL", "transport": "ui", "ready": true},
                            {"id": "watcher@GLOBAL", "transport": "ui", "ready": true},
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
    let relay = FakeRelay::start_for_bundles(runtime.relay_socket.clone(), routes);
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "list", list_sessions_call(Map::new()))
        .await;
    let payload = decode_tool_payload(&response);
    let bundles = payload["bundles"]
        .as_array()
        .expect("bundles must be array");

    assert_eq!(bundles.len(), 2);
    assert_eq!(bundles[0]["id"], BUNDLE_NAME);
    let global = &bundles[1];
    assert_eq!(global["id"], "GLOBAL");
    assert_eq!(global["hosted"], true);
    assert_eq!(global["state"], "up");
    assert_eq!(global["sessions"][0]["id"], "operator@GLOBAL");
    assert_eq!(global["sessions"][0]["transport"], "ui");
    assert_eq!(global["sessions"][0]["ready"], true);
    assert_eq!(global["sessions"][1]["id"], "watcher@GLOBAL");

    let envelopes = relay.envelopes_for_operation("list");
    let namespaces = envelopes
        .iter()
        .filter_map(|envelope| envelope["namespace"].as_str())
        .collect::<Vec<_>>();
    assert!(namespaces.contains(&BUNDLE_NAME));
    assert!(namespaces.contains(&"GLOBAL"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_all_mode_fails_fast_on_first_authorization_denial() {
    let runtime = TestRuntime::create();
    write_bundle_configuration(&runtime.config_root, "alpha", &["alpha"]);
    write_bundle_configuration(&runtime.config_root, "zeta", &["zeta"]);
    let mut routes: HashMap<String, RelayResponder> = HashMap::new();
    routes.insert(
        "alpha".to_string(),
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
    routes.insert(
        BUNDLE_NAME.to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": BUNDLE_NAME,
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [{"id": BUNDLE_NAME, "transport": "tmux", "ready": true}],
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
    routes.insert(
        "zeta".to_string(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle": {
                        "id": "zeta",
                        "hosted": true,
                        "state": "up",
                        "startup_health": "healthy",
                        "state_reason_code": null,
                        "state_reason": null,
                        "startup_failure_count": 0,
                        "recent_startup_failures": [],
                        "sessions": [{"id": "zeta", "transport": "tmux", "ready": true}],
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
    let relay = FakeRelay::start_for_bundles(runtime.relay_socket.clone(), routes);
    let mut harness = McpHarness::spawn(&runtime).await;
    let arguments = list_sessions_call(Map::from_iter([("all".to_string(), Value::Bool(true))]));
    let response = harness.call_tool(2, "list", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    let list_requests = relay.requests_for_operation("list");
    assert_eq!(list_requests.len(), 1);
}
