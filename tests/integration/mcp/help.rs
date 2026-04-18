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
        4
    );
    assert_eq!(payload["tools"][0]["tool"], "list");
    assert_eq!(payload["tools"][0]["kind"], "meta_tool");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_query_returns_meta_tool_command_catalog() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "help", help_call(Some("list"))).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["tool"], "list");
    assert_eq!(payload["kind"], "meta_tool");
    assert_eq!(payload["commands"][0]["command"], "list.sessions");
    assert_eq!(payload["invoke"]["tool"], "list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_list_sessions_query_returns_args_schema() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("list.sessions")))
        .await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["command"], "list.sessions");
    assert!(payload["args_schema"]["properties"]["bundle_name"].is_object());
    assert!(payload["args_schema"]["properties"]["all"].is_object());
    assert_eq!(payload["invoke"]["tool"], "list");
    assert_eq!(payload["invoke"]["params"]["command"], "sessions");
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
async fn help_rejects_unknown_query() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness
        .call_tool(2, "help", help_call(Some("list.bundles")))
        .await;

    assert_eq!(error_code(&response), Some("validation_invalid_params"));
}
