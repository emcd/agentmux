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

fn drop_peer_arguments(principal_id: &str) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("peer".to_string()));
    arguments.insert("args".to_string(), json!({"principal_id": principal_id}));
    arguments
}

/// Responder answering `drop_peer` for a session principal, which is the one
/// principal type the relay owns a canonical credential location for.
fn drop_session_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("drop_peer") => json!({
                "kind": "drop_peer",
                "schema_version": "1",
                "principal_id": "worker@party",
                "principal_type": "session",
                "credential_path": "/state/sessions/worker/identity.psk",
            }),
            _ => json!({
                "kind": "error",
                "error": {"code": "internal_unexpected_failure", "message": "unexpected operation"},
            }),
        },
    )
}

/// Responder answering `drop_peer` for a peer relay, whose credential lives
/// under the connecting relay's state root and so carries no path.
fn drop_relay_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("drop_peer") => json!({
                "kind": "drop_peer",
                "schema_version": "1",
                "principal_id": "rnd-main@RELAY",
                "principal_type": "relay",
                "credential_path": null,
            }),
            _ => json!({
                "kind": "error",
                "error": {"code": "internal_unexpected_failure", "message": "unexpected operation"},
            }),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_peer_forwards_the_principal_id_to_the_relay() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(runtime.relay_socket.clone(), drop_session_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    harness
        .call_tool(2, "drop", drop_peer_arguments("worker@party"))
        .await;

    let requests = relay.requests_for_operation("drop_peer");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["principal_id"], "worker@party");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_peer_reports_the_credential_path_for_a_session_principal() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), drop_session_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "drop", drop_peer_arguments("worker@party"))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["principal_id"], "worker@party");
    assert_eq!(payload["principal_type"], "session");
    assert_eq!(
        payload["credential_path"], "/state/sessions/worker/identity.psk",
        "a session principal's relay-owned credential path must be reported: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_peer_omits_the_credential_path_for_a_peer_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), drop_relay_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "drop", drop_peer_arguments("rnd-main@RELAY"))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["principal_type"], "relay");
    assert!(
        payload.get("credential_path").is_none(),
        "a peer relay's credential lives on the connecting relay, so no path may \
         be reported: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_rejects_a_command_other_than_peer() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay must not receive a drop with an unknown command")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("command".to_string(), Value::String("psk".to_string()));
    arguments.insert("args".to_string(), json!({"principal_id": "worker@party"}));
    let response = harness.call_tool(2, "drop", arguments).await;

    assert_eq!(
        response["error"]["data"]["code"], "validation_invalid_params",
        "unexpected response: {response}"
    );
}

/// Responder answering `new_peer` with the policy-tier scope advisory attached,
/// matching how the relay serializes a diagnostic-bearing success.
fn scope_advisory_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => json!({
                "kind": "new_peer",
                "schema_version": "1",
                "principal_id": "rnd-main@RELAY",
                "principal_type": "relay",
                "psk": "SECRET-PSK",
                "written_path": null,
                "config_snippet": "# snippet",
                "diagnostics": [{
                    "code": "advisory_scope_resembles_policy_tier",
                    "message": "ingress scope 'all' is a policy-tier value",
                }],
            }),
            _ => json!({
                "kind": "error",
                "error": {"code": "internal_unexpected_failure", "message": "unexpected operation"},
            }),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_preserves_a_scope_diagnostic_in_the_structured_result() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), scope_advisory_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "new", peer_args(json!({"scope": "all"})))
        .await;
    let payload = decode_tool_payload(&response);

    // The advisory has to reach an MCP caller through the payload: relay stderr
    // is a different process's stream, and MCP has no stderr channel at all.
    assert_eq!(
        payload["diagnostics"][0]["code"], "advisory_scope_resembles_policy_tier",
        "the scope advisory must survive into the structured result: {payload}"
    );
    assert!(
        payload["diagnostics"][0]["message"].is_string(),
        "each diagnostic carries a human-readable message: {payload}"
    );
    assert_eq!(
        payload["psk"], "SECRET-PSK",
        "the advisory must not disturb the credential payload: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_peer_omits_diagnostics_when_none_were_raised() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(runtime.relay_socket.clone(), response_mode_responder());
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness
        .call_tool(2, "new", peer_args(json!({"scope": "myapp"})))
        .await;
    let payload = decode_tool_payload(&response);

    assert!(
        payload.get("diagnostics").is_none(),
        "diagnostics must be omitted rather than rendered as an empty array: {payload}"
    );
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
