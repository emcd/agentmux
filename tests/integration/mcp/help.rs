use serde_json::{Map, Value};

use super::helpers::*;

fn help_call(query: Option<&str>) -> Map<String, Value> {
    let mut args = Map::new();
    if let Some(value) = query {
        args.insert("query".to_string(), Value::String(value.to_string()));
    }
    args
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_without_query_returns_tool_inventory() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(None)).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["namespace"], "agentmux");
    assert_eq!(
        payload["tools"].as_array().map_or(0, |value| value.len()),
        10
    );
    assert_eq!(payload["tools"][0]["tool"], "list");
    assert_eq!(payload["tools"][0]["kind"], "meta_tool");
    let tools = payload["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|entry| entry["tool"] == "drop"),
        "the inventory must advertise drop: {tools:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_drop_query_returns_meta_tool_command_catalog() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("drop"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["tool"], "drop");
    assert_eq!(payload["kind"], "meta_tool");
    assert_eq!(payload["commands"][0]["command"], "drop.peer");
    assert_eq!(payload["invoke"]["tool"], "drop");
    assert_eq!(payload["invoke"]["params"]["command"], "peer");
}

// The drop tool's own invalid-args error tells the caller to run
// `help query 'drop.peer'`, so that query has to resolve or the hint sends them
// into a second error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_drop_peer_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("drop.peer")))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "drop.peer");
    assert_eq!(payload["invoke"]["tool"], "drop");
    assert!(
        payload["args_schema"]["properties"]
            .get("principal_id")
            .is_some(),
        "drop.peer schema must document principal_id: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_query_returns_meta_tool_command_catalog() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("list"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["tool"], "list");
    assert_eq!(payload["kind"], "meta_tool");
    assert_eq!(payload["commands"][0]["command"], "list.principals");
    let commands = payload["commands"].as_array().expect("commands array");
    assert!(
        commands
            .iter()
            .any(|entry| entry["command"] == "list.decisions"),
        "list catalog includes list.decisions: {commands:?}"
    );
    assert_eq!(payload["invoke"]["tool"], "list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_catalog_and_discovery_schemas_advertise_relay_and_namespace() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;

    // The list catalog advertises the discovery commands.
    let catalog = decode_tool_payload(&harness.call_tool(2, "help", help_call(Some("list"))).await);
    let commands = catalog["commands"].as_array().expect("commands array");
    for expected in ["list.namespaces", "list.relays"] {
        assert!(
            commands.iter().any(|entry| entry["command"] == expected),
            "list catalog includes {expected}: {commands:?}"
        );
    }

    // list.namespaces marks `relay` optional and takes no other properties.
    let namespaces = decode_tool_payload(
        &harness
            .call_tool(3, "help", help_call(Some("list.namespaces")))
            .await,
    );
    assert_eq!(namespaces["invoke"]["params"]["command"], "namespaces");
    assert!(namespaces["args_schema"]["properties"]["relay"].is_object());
    assert!(namespaces["args_schema"]["properties"]["namespace"].is_null());

    // list.relays takes no arguments.
    let relays = decode_tool_payload(
        &harness
            .call_tool(4, "help", help_call(Some("list.relays")))
            .await,
    );
    assert_eq!(relays["invoke"]["params"]["command"], "relays");
    assert!(
        relays["args_schema"]["properties"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
    );

    // list.principals advertises both the namespace and the added relay selector.
    let principals = decode_tool_payload(
        &harness
            .call_tool(5, "help", help_call(Some("list.principals")))
            .await,
    );
    assert!(principals["args_schema"]["properties"]["namespace"].is_object());
    assert!(principals["args_schema"]["properties"]["relay"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_decisions_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("list.decisions")))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "list.decisions");
    assert!(payload["args_schema"].is_object());
    assert_eq!(payload["invoke"]["tool"], "list");
    assert_eq!(payload["invoke"]["params"]["command"], "decisions");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_choose_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("choose")))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "choose");
    assert!(payload["args_schema"]["properties"]["choice_request_id"].is_object());
    assert!(payload["args_schema"]["properties"]["outcome"].is_object());
    assert!(payload["args_schema"]["properties"]["option_id"].is_object());
    assert_eq!(payload["invoke"]["tool"], "choose");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_principals_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("list.principals")))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "list.principals");
    assert!(payload["args_schema"]["properties"]["namespace"].is_object());
    assert_eq!(payload["invoke"]["tool"], "list");
    assert_eq!(payload["invoke"]["params"]["command"], "principals");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_send_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("send"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "send");
    assert!(payload["args_schema"]["properties"]["message"].is_object());
    assert!(payload["args_schema"]["properties"]["targets"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_raww_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("raww"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "raww");
    assert!(payload["args_schema"]["properties"]["target_session"].is_object());
    assert!(payload["args_schema"]["properties"]["text"].is_object());
    assert!(payload["args_schema"]["properties"]["no_enter"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_rejects_unknown_fields() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(
            2,
            "help",
            Map::from_iter([("unexpected".to_string(), Value::String("value".to_string()))]),
        )
        .await;

    assert_unknown_field_error(&response, &["unexpected"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_schemas_disallow_additional_properties() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("send"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["args_schema"]["additionalProperties"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_rejects_unknown_query() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("list.bundles")))
        .await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
}
