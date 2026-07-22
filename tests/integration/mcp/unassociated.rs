//! An MCP server started without a resolvable bundle+session holds no relay
//! stream, so every relay-backed tool rejects the call with the single
//! actionable `validation_unassociated_server` contract: a validation-shaped
//! error carrying a canonical reason and remedy.

use serde_json::{Map, Value, json};

use super::helpers::*;

/// The exact remedy string the server advertises for an unassociated MCP
/// server, shared by the error `details`, the help association block, and the
/// `get_info` instructions.
const CANONICAL_REMEDY: &str = "agentmux host mcp --bundle <name> --session-name <id>";

/// Asserts a response is the canonical unassociated-server rejection: a
/// validation-shaped JSON-RPC error (`invalid_params`, not `internal_error`)
/// coded `validation_unassociated_server` and carrying the actionable remedy.
fn assert_unassociated(response: &Value) {
    assert_eq!(
        response["error"]["code"].as_i64(),
        Some(-32602),
        "unassociated error must be validation-shaped (invalid_params): {response}"
    );
    assert_eq!(
        error_code(response),
        Some("validation_unassociated_server"),
        "unexpected error code: {response}"
    );
    assert_eq!(
        response["error"]["data"]["details"]["reason"], "unassociated_server",
        "missing canonical reason: {response}"
    );
    assert_eq!(
        response["error"]["data"]["details"]["remedy"], CANONICAL_REMEDY,
        "remedy detail must equal the canonical remedy: {response}"
    );
}

fn arguments(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unassociated_server_rejects_every_relay_backed_tool() {
    let runtime = TestRuntime::create();
    // No relay is started: an unassociated server must fail on the absent stream
    // before any relay contact, so the outcome cannot depend on a live relay.
    let mut harness = McpHarness::spawn_unassociated(&runtime).await;

    // Prechecked tools that need the sender value.
    let send = harness
        .call_tool(
            2,
            "send",
            arguments([
                ("message", Value::String("hello".to_string())),
                (
                    "targets",
                    Value::Array(vec![Value::String("bravo".to_string())]),
                ),
            ]),
        )
        .await;
    assert_unassociated(&send);

    let look = harness
        .call_tool(
            3,
            "look",
            arguments([("target_session", Value::String("bravo".to_string()))]),
        )
        .await;
    assert_unassociated(&look);

    let raww = harness
        .call_tool(
            4,
            "raww",
            arguments([
                ("target_session", Value::String("bravo".to_string())),
                ("text", Value::String("ls".to_string())),
            ]),
        )
        .await;
    assert_unassociated(&raww);

    // Relay-wide tools reach the absent stream through the
    // `map_relay_stream_failure` chokepoint.
    let choose = harness
        .call_tool(
            5,
            "choose",
            arguments([
                ("choice_request_id", Value::String("req-1".to_string())),
                ("outcome", Value::String("cancelled".to_string())),
            ]),
        )
        .await;
    assert_unassociated(&choose);

    let updown = harness
        .call_tool(
            6,
            "updown",
            arguments([("command", Value::String("up".to_string()))]),
        )
        .await;
    assert_unassociated(&updown);

    let new = harness
        .call_tool(
            7,
            "new",
            arguments([
                ("command", Value::String("peer".to_string())),
                ("args", json!({"principal_id": "scout@myns"})),
            ]),
        )
        .await;
    assert_unassociated(&new);

    let change = harness
        .call_tool(
            8,
            "change",
            arguments([
                ("command", Value::String("psk".to_string())),
                ("args", json!({"principal_id": "scout@myns"})),
            ]),
        )
        .await;
    assert_unassociated(&change);

    // Relay-wide `list` discovery commands share the same precondition.
    let relays = harness
        .call_tool(
            9,
            "list",
            arguments([("command", Value::String("relays".to_string()))]),
        )
        .await;
    assert_unassociated(&relays);

    let namespaces = harness
        .call_tool(
            10,
            "list",
            arguments([("command", Value::String("namespaces".to_string()))]),
        )
        .await;
    assert_unassociated(&namespaces);

    let principals = harness
        .call_tool(
            11,
            "list",
            arguments([("command", Value::String("principals".to_string()))]),
        )
        .await;
    assert_unassociated(&principals);

    let decisions = harness
        .call_tool(
            12,
            "list",
            arguments([("command", Value::String("decisions".to_string()))]),
        )
        .await;
    assert_unassociated(&decisions);

    // Foreign principal discovery (relay selector) routes through the same
    // association guard before it would forward to a configured peer.
    let foreign_principals = harness
        .call_tool(
            13,
            "list",
            arguments([
                ("command", Value::String("principals".to_string())),
                ("args", json!({"relay": "west", "namespace": "myapp"})),
            ]),
        )
        .await;
    assert_unassociated(&foreign_principals);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unassociated_server_help_and_get_info_surface_remedy() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn_unassociated(&runtime).await;

    // The get_info instructions captured at initialize name the failure code
    // and carry the exact remedy, so the hint cannot silently regress.
    assert!(
        harness
            .instructions()
            .contains("validation_unassociated_server"),
        "unassociated instructions must name the error: {:?}",
        harness.instructions()
    );
    assert!(
        harness.instructions().contains(CANONICAL_REMEDY),
        "unassociated instructions must carry the remedy: {:?}",
        harness.instructions()
    );

    let payload = decode_tool_payload(&harness.call_tool(2, "help", Map::new()).await);
    assert_eq!(payload["association"]["associated"], false);
    assert_eq!(payload["association"]["remedy"], CANONICAL_REMEDY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn associated_server_help_and_get_info_surface_association() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;

    // The associated instructions name the bound namespace and session.
    assert!(
        harness.instructions().contains(BUNDLE_NAME),
        "associated instructions must name the namespace: {:?}",
        harness.instructions()
    );
    assert!(
        harness.instructions().contains(SENDER_SESSION),
        "associated instructions must name the session: {:?}",
        harness.instructions()
    );

    let payload = decode_tool_payload(&harness.call_tool(2, "help", Map::new()).await);
    assert_eq!(payload["association"]["associated"], true);
    assert_eq!(payload["association"]["namespace"], BUNDLE_NAME);
    assert_eq!(payload["association"]["session"], SENDER_SESSION);
}
