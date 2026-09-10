//! `link` tool: the MCP adapter folds the `alias` / `psk` args into the
//! relay `InstallPeerCredential` selector and never renders the secret.

use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

fn link_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("install_peer_credential") => json!({
                "kind": "install_peer_credential",
                "schema_version": "1",
                "alias": request.get("alias").cloned().unwrap_or(Value::Null),
                "written_path": "/tmp/state/peers/bravo.psk",
            }),
            _ => json!({
                "kind": "error",
                "error": {
                    "code": "internal_unexpected_failure",
                    "message": "unexpected operation",
                },
            }),
        },
    )
}

fn link_args(extra: Value) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("peer".to_string()));
    let mut args = json!({"alias": "bravo", "psk": "issued-secret"});
    if let (Value::Object(args_map), Value::Object(extra_map)) = (&mut args, extra) {
        args_map.extend(extra_map);
    }
    arguments.insert("args".to_string(), args);
    arguments
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_peer_installs_into_relay_owned_slot() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), link_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness.call_tool(2, "link", link_args(json!({}))).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["alias"], "bravo");
    assert_eq!(payload["written_path"], "/tmp/state/peers/bravo.psk");
    assert!(
        payload.get("psk").is_none(),
        "link response must never carry the PSK: {payload}"
    );

    let requests = relay.requests_for_operation("install_peer_credential");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["alias"], "bravo");
    assert_eq!(requests[0]["psk"], "issued-secret");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_peer_rejects_missing_alias_before_relay_contact() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive link for a missing alias")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut args = link_args(json!({}));
    if let Some(Value::Object(args_map)) = args.get_mut("args") {
        args_map.remove("alias");
    }
    let response = harness.call_tool(2, "link", args).await;
    let error = response["error"].as_object().expect("tool error");
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("alias is required"),
        "unexpected error: {response:?}"
    );
    assert!(
        relay
            .requests_for_operation("install_peer_credential")
            .is_empty(),
        "rejected link must not reach the relay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_peer_rejects_missing_psk_before_relay_contact() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive link for a missing psk")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut args = link_args(json!({}));
    if let Some(Value::Object(args_map)) = args.get_mut("args") {
        args_map.remove("psk");
    }
    let response = harness.call_tool(2, "link", args).await;
    let error = response["error"].as_object().expect("tool error");
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("psk is required"),
        "unexpected error: {response:?}"
    );
    assert!(
        relay
            .requests_for_operation("install_peer_credential")
            .is_empty(),
        "rejected link must not reach the relay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_peer_rejects_unknown_command() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive link for an unknown command")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("rotate".to_string()));
    arguments.insert("args".to_string(), json!({}));
    let response = harness.call_tool(2, "link", arguments).await;
    let error = response["error"].as_object().expect("tool error");
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("link command must be"),
        "unexpected error: {response:?}"
    );
    assert!(
        relay
            .requests_for_operation("install_peer_credential")
            .is_empty(),
        "rejected link must not reach the relay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_peer_surfaces_relay_rejection() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| {
            json!({
                "kind": "error",
                "error": {
                    "code": "validation_unknown_principal",
                    "message": "no relay principal registered for alias",
                },
            })
        }),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness.call_tool(2, "link", link_args(json!({}))).await;
    let error = response["error"].as_object().expect("tool error");
    assert_eq!(
        error
            .get("data")
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str),
        Some("validation_unknown_principal"),
        "relay rejection must surface: {response:?}"
    );
    assert_eq!(
        relay
            .requests_for_operation("install_peer_credential")
            .len(),
        1
    );
}
