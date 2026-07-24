//! `new` / `change` credential-destination translation: the MCP adapter folds
//! the ergonomic `output_path` / `write_to_config` args into the relay
//! `CredentialDestination` selector and rejects the mutually-exclusive
//! both-set case before any relay request is issued.

use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// Responder that answers `new_peer` / `change_psk` with a canned success so the
/// adapter completes; the assertions inspect the recorded relay request, not the
/// response.
fn credential_admin_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => json!({
                "kind": "new_peer",
                "schema_version": "1",
                "principal_id": "worker@party",
                "principal_type": "session",
                "psk": null,
                "written_path": "/tmp/cred.psk",
                "config_snippet": "# snippet",
            }),
            Some("change_psk") => json!({
                "kind": "change_psk",
                "schema_version": "1",
                "principal_id": "worker@party",
                "psk": null,
                "written_path": "/tmp/cred.psk",
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

fn peer_args(extra: Value) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("peer".to_string()));
    let mut args = json!({"principal_id": "worker@party"});
    if let (Value::Object(args_map), Value::Object(extra_map)) = (&mut args, extra) {
        args_map.extend(extra_map);
    }
    arguments.insert("args".to_string(), args);
    arguments
}

fn psk_args(extra: Value) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("psk".to_string()));
    let mut args = json!({"principal_id": "worker@party"});
    if let (Value::Object(args_map), Value::Object(extra_map)) = (&mut args, extra) {
        args_map.extend(extra_map);
    }
    arguments.insert("args".to_string(), args);
    arguments
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_output_path_translates_to_path_destination() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = peer_args(json!({"output_path": "/tmp/cred.psk"}));
    harness.call_tool(2, "new", arguments).await;

    let requests = relay.requests_for_operation("new_peer");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["destination"],
        json!({"kind": "path", "path": "/tmp/cred.psk"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_write_to_config_translates_to_config_destination() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = peer_args(json!({"write_to_config": true}));
    harness.call_tool(2, "new", arguments).await;

    let requests = relay.requests_for_operation("new_peer");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["destination"], json!({"kind": "config"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_default_destination_is_response() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = peer_args(json!({}));
    harness.call_tool(2, "new", arguments).await;

    let requests = relay.requests_for_operation("new_peer");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["destination"], json!({"kind": "response"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_rejects_output_path_and_write_to_config_together() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive new_peer for a mutually-exclusive request")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = peer_args(json!({"output_path": "/tmp/cred.psk", "write_to_config": true}));
    let response = harness.call_tool(2, "new", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("new_peer").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_psk_output_path_translates_to_path_destination() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = psk_args(json!({"output_path": "/tmp/cred.psk"}));
    harness.call_tool(2, "change", arguments).await;

    let requests = relay.requests_for_operation("change_psk");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["destination"],
        json!({"kind": "path", "path": "/tmp/cred.psk"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_psk_write_to_config_translates_to_config_destination() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = psk_args(json!({"write_to_config": true}));
    harness.call_tool(2, "change", arguments).await;

    let requests = relay.requests_for_operation("change_psk");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["destination"], json!({"kind": "config"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_psk_rejects_output_path_and_write_to_config_together() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive change_psk for a mutually-exclusive request")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let arguments = psk_args(json!({"output_path": "/tmp/cred.psk", "write_to_config": true}));
    let response = harness.call_tool(2, "change", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("change_psk").is_empty());
}

/// Responder answering `new_peer` in Response mode (psk present, no written
/// path), matching how the relay serializes a Response-destination result.
fn response_mode_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => json!({
                "kind": "new_peer",
                "schema_version": "1",
                "principal_id": "worker@party",
                "principal_type": "session",
                "psk": "SECRET-PSK",
                "written_path": null,
                "config_snippet": "# snippet",
            }),
            Some("change_psk") => json!({
                "kind": "change_psk",
                "schema_version": "1",
                "principal_id": "worker@party",
                "psk": "SECRET-PSK",
                "written_path": null,
            }),
            _ => json!({
                "kind": "error",
                "error": {"code": "internal_unexpected_failure", "message": "unexpected operation"},
            }),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_response_mode_payload_omits_written_path() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), response_mode_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness.call_tool(2, "new", peer_args(json!({}))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["psk"], "SECRET-PSK");
    assert!(
        payload.get("written_path").is_none(),
        "written_path must be omitted in Response mode: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_written_payload_omits_psk() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "new", peer_args(json!({"write_to_config": true})))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["written_path"], "/tmp/cred.psk");
    assert!(
        payload.get("psk").is_none(),
        "psk must be omitted when written to a file: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_psk_written_payload_omits_psk() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "change", psk_args(json!({"write_to_config": true})))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["written_path"], "/tmp/cred.psk");
    assert!(
        payload.get("psk").is_none(),
        "psk must be omitted when written to a file: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_empty_output_path_with_config_is_mutually_exclusive() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive new_peer for a mutually-exclusive request")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    // An explicitly-supplied (even empty) output_path conflicts with
    // write_to_config: presence, not emptiness, drives mutual exclusion.
    let arguments = peer_args(json!({"output_path": "", "write_to_config": true}));
    let response = harness.call_tool(2, "new", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
    assert!(relay.requests_for_operation("new_peer").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_empty_output_path_forwards_as_path_for_relay_rejection() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), credential_admin_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    // An empty output_path must forward as a Path destination (for the relay to
    // reject as validation_invalid_output_path), not silently degrade to
    // Response.
    let arguments = peer_args(json!({"output_path": ""}));
    harness.call_tool(2, "new", arguments).await;

    let requests = relay.requests_for_operation("new_peer");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["destination"],
        json!({"kind": "path", "path": ""})
    );
}
