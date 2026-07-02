use super::helpers::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_returns_snapshot_payload_and_forwards_request_shape() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_format": "lines",
                    "snapshot_lines": ["LOOK-A", "LOOK-B", "LOOK-C"],
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("lines".to_string(), Value::Number(3.into()));
    let response = harness.call_tool(2, "look", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["requester_session"], SENDER_SESSION);
    assert_eq!(payload["target_session"], "bravo");
    assert_eq!(payload["snapshot_format"], "lines");
    assert_eq!(
        payload["snapshot_lines"],
        Value::Array(vec![
            Value::String("LOOK-A".to_string()),
            Value::String("LOOK-B".to_string()),
            Value::String("LOOK-C".to_string()),
        ])
    );
    assert!(payload.get("snapshot_entries").is_none());
    assert!(payload.get("freshness").is_none());
    assert!(payload.get("snapshot_source").is_none());
    assert!(payload.get("stale_reason_code").is_none());
    assert!(payload.get("snapshot_age_ms").is_none());
    assert!(payload.get("authenticated_identity").is_none());
    assert!(payload.get("on_behalf_of").is_none());

    let relay_requests = relay.requests_for_operation("look");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["requester_session"], SENDER_SESSION);
    assert_eq!(relay_requests[0]["target_session"], "bravo");
    assert_eq!(relay_requests[0]["lines"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_forwards_offset_to_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_format": "structured_entries_v1",
                    "snapshot_entries": [{"kind": "agent", "lines": ["older context"]}],
                    "entries_total": 10,
                    "returned_entries_count": 1,
                    "freshness": "fresh",
                    "snapshot_source": "live_buffer",
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("lines".to_string(), Value::Number(1.into()));
    arguments.insert("offset".to_string(), Value::Number(3.into()));
    let response = harness.call_tool(2, "look", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["entries_total"], 10);
    assert_eq!(payload["returned_entries_count"], 1);

    let relay_requests = relay.requests_for_operation("look");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["lines"], 1);
    assert_eq!(relay_requests[0]["offset"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_preserves_additive_acp_freshness_fields() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_format": "structured_entries_v1",
                    "snapshot_entries": [],
                    "entries_total": 0,
                    "returned_entries_count": 0,
                    "freshness": "stale",
                    "snapshot_source": "none",
                    "stale_reason_code": "acp_snapshot_prime_timeout",
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["snapshot_format"], "structured_entries_v1");
    assert_eq!(payload["snapshot_entries"], json!([]));
    assert!(payload.get("snapshot_lines").is_none());
    assert_eq!(payload["freshness"], "stale");
    assert_eq!(payload["snapshot_source"], "none");
    assert_eq!(payload["stale_reason_code"], "acp_snapshot_prime_timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_preserves_structured_acp_entries_passthrough() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_format": "structured_entries_v1",
                    "snapshot_entries": [
                        {
                            "kind": "user",
                            "lines": ["Please summarize the changes."]
                        },
                        {
                            "kind": "invocation",
                            "call_id": "call_0",
                            "status": "Pending",
                            "invocation": {"tool": "search", "query": "agentmux changelog"},
                            "result": null
                        },
                        {
                            "kind": "agent",
                            "lines": ["Summary generated."]
                        },
                        {
                            "kind": "update",
                            "update_kind": "status",
                            "lines": ["done"]
                        }
                    ],
                    "entries_total": 12,
                    "returned_entries_count": 4,
                    "freshness": "fresh",
                    "snapshot_source": "live_buffer",
                    "snapshot_age_ms": 42,
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["snapshot_format"], "structured_entries_v1");
    assert_eq!(
        payload["snapshot_entries"],
        json!([
            {
                "kind": "user",
                "lines": ["Please summarize the changes."]
            },
            {
                "kind": "invocation",
                "call_id": "call_0",
                "status": "Pending",
                "invocation": {"tool": "search", "query": "agentmux changelog"},
                "result": null
            },
            {
                "kind": "agent",
                "lines": ["Summary generated."]
            },
            {
                "kind": "update",
                "update_kind": "status",
                "lines": ["done"]
            }
        ])
    );
    assert!(payload.get("snapshot_lines").is_none());
    assert_eq!(payload["freshness"], "fresh");
    assert_eq!(payload["snapshot_source"], "live_buffer");
    assert_eq!(payload["snapshot_age_ms"], 42);
    assert!(payload.get("stale_reason_code").is_none());
    assert_eq!(payload["entries_total"], 12);
    assert_eq!(payload["returned_entries_count"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_rejects_invalid_lines_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive look request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert("lines".to_string(), Value::Number(0.into()));
    let response = harness.call_tool(2, "look", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_lines"));
    assert!(relay.requests_for_operation("look").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_rejects_unknown_fields_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive look request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    arguments.insert(
        "requester_session".to_string(),
        Value::String("spoof".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;

    assert_unknown_field_error(&response, &["requester_session"]);
    assert!(relay.requests_for_operation("look").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_maps_validation_errors_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "error",
                    "error": {
                        "code": "validation_unknown_target",
                        "message": "target_session is not in bundle configuration",
                        "details": {"target_session": "missing"},
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("missing".to_string()),
    );
    arguments.insert("lines".to_string(), Value::Number(120.into()));
    let response = harness.call_tool(2, "look", arguments).await;

    assert_eq!(error_code(&response), Some("validation_unknown_target"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_maps_authorization_forbidden_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "error",
                    "error": {
                        "code": "authorization_forbidden",
                        "message": "request denied by authorization policy",
                        "details": {
                            "capability": "look.inspect",
                            "requester_session": SENDER_SESSION,
                            "bundle_name": BUNDLE_NAME,
                            "target_session": "bravo",
                            "reason": "look is restricted by policy",
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;

    assert_eq!(error_code(&response), Some("authorization_forbidden"));
    assert_eq!(
        response["error"]["data"]["details"]["capability"],
        "look.inspect"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_maps_unknown_peer_bundle_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "error",
                    "error": {
                        "code": "validation_unknown_bundle",
                        "message": "look target bundle is not configured on this relay",
                        "details": {
                            "bundle_name": "other",
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

    // The MCP `look` tool forwards a `session@bundle` target verbatim; the relay
    // now resolves it cross-bundle and the peer-bundle validation error maps
    // through unchanged.
    let mut arguments = Map::new();
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo@other".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;

    assert_eq!(error_code(&response), Some("validation_unknown_bundle"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_forwards_bound_bundle_on_wire_envelope() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": SENDER_SESSION,
                    "target_session": "bravo",
                    "captured_at": "2026-03-10T00:00:00Z",
                    "snapshot_format": "lines",
                    "snapshot_lines": [],
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;
    decode_tool_payload(&response);

    let envelopes = relay.envelopes_for_operation("look");
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["namespace"], BUNDLE_NAME);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn look_surfaces_authenticated_identity_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("look") => json!({
                    "kind": "look",
                    "schema_version": "1",
                    "requester_session": request.get("requester_session").cloned().unwrap_or(Value::Null),
                    "target_session": request.get("target_session").cloned().unwrap_or(Value::Null),
                    "captured_at": "2026-03-10T00:00:00Z",
                    "authenticated_identity": "alpha@agentmux",
                    "snapshot_format": "lines",
                    "snapshot_lines": ["LOOK-A"],
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
    arguments.insert(
        "target_session".to_string(),
        Value::String("bravo".to_string()),
    );
    let response = harness.call_tool(2, "look", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["authenticated_identity"], "alpha@agentmux");
    // `on_behalf_of` is reserved and absent on the wire, so it is omitted
    // entirely rather than surfaced as null.
    assert!(payload.get("on_behalf_of").is_none());
}
