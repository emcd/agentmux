//! MiMo-compatibility guard for MCP tool schemas.
//!
//! The Xiaomi/MiMo tool-call serializer truncates the argument JSON at the first
//! parameter whose JSON-Schema `type` is a nullable union (`["...", "null"]`),
//! so every optional field must render as a plain single type and stay out of
//! `required` rather than as an `Option`-style null-union. Both schema emission
//! paths are covered: the rmcp `#[tool]` `inputSchema` advertised in the tool
//! inventory, and the `help` tool's `args_schema` produced via `schema_for!`.
//! See nb `artifacts/mcp/1` for the full analysis.

use serde_json::Value;

use super::helpers::*;

/// Records the path of any schema node whose `type` is an array containing
/// `"null"` (a nullable union).
fn collect_null_unions(node: &Value, path: &str, found: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Array(variants)) = map.get("type")
                && variants.iter().any(|variant| variant == "null")
            {
                found.push(path.to_string());
            }
            for (key, child) in map {
                collect_null_unions(child, &format!("{path}/{key}"), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_null_unions(child, &format!("{path}/{index}"), found);
            }
        }
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_input_schemas_are_free_of_null_unions() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;

    let response = harness.list_tools(2).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tool list array");
    assert!(!tools.is_empty(), "expected advertised tools");

    let mut offenders = Vec::new();
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        if let Some(schema) = tool.get("inputSchema") {
            collect_null_unions(schema, name, &mut offenders);
        }
    }
    assert!(
        offenders.is_empty(),
        "tool inputSchema null-unions break MiMo tool-call serialization: {offenders:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn help_command_schemas_are_free_of_null_unions() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;

    // Queries whose help payload carries an `args_schema` built from an
    // optional-bearing param/arg struct.
    let queries = [
        "send",
        "look",
        "raww",
        "choose",
        "help",
        "list.principals",
        "new.peer",
        "change.psk",
    ];

    let mut offenders = Vec::new();
    for (index, query) in queries.iter().enumerate() {
        let arguments = serde_json::Map::from_iter([(
            "query".to_string(),
            Value::String((*query).to_string()),
        )]);
        let response = harness.call_tool(2 + index as i64, "help", arguments).await;
        let payload = decode_tool_payload(&response);
        let schema = payload
            .get("args_schema")
            .unwrap_or_else(|| panic!("help query '{query}' missing args_schema: {payload}"));
        collect_null_unions(schema, query, &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "help args_schema null-unions break MiMo tool-call serialization: {offenders:?}"
    );
}
