//! `look` tool: inspect a target session pane snapshot for this bundle.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::{Value, json};

use crate::relay::{LookSnapshotPayload, RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::{internal_tool_error, map_relay_error, validation_tool_error};
use crate::mcp::params::LookParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::validate_look_request;

#[tool_router(router = tool_router_look, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "look",
        description = "Inspect a target session pane snapshot for this bundle."
    )]
    async fn tool_look(
        &self,
        Parameters(params): Parameters<LookParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_look_request(&params)?;
        emit_inscription(
            "mcp.tool.look.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "requester_session": self.state.configuration.sender_session.clone(),
                "target_session": params.target_session.clone(),
                "lines": params.lines,
                "offset": params.offset,
            }),
        );
        let requester_session = self
            .state
            .configuration
            .sender_session
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_unknown_sender",
                    "sender session is not configured for this MCP server",
                    None,
                )
            })?;

        let request = RelayRequest::Look {
            requester_session,
            target_session: params.target_session.clone(),
            lines: params.lines.map(|value| value as usize),
            offset: params.offset.map(|value| value as usize),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Look {
                schema_version,
                requester_session,
                target_session,
                captured_at,
                authenticated_identity,
                on_behalf_of,
                snapshot,
            }) => {
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                response_map.insert(
                    "requester_session".to_string(),
                    Value::String(requester_session),
                );
                response_map.insert("target_session".to_string(), Value::String(target_session));
                response_map.insert("captured_at".to_string(), Value::String(captured_at));
                if let Some(identity) = authenticated_identity {
                    response_map.insert(
                        "authenticated_identity".to_string(),
                        Value::String(identity),
                    );
                }
                if let Some(delegate) = on_behalf_of {
                    response_map.insert("on_behalf_of".to_string(), Value::String(delegate));
                }
                let snapshot_count = match snapshot {
                    LookSnapshotPayload::Lines { snapshot_lines } => {
                        let count = snapshot_lines.len();
                        response_map.insert("snapshot_format".to_string(), json!("lines"));
                        response_map.insert("snapshot_lines".to_string(), json!(snapshot_lines));
                        count
                    }
                    LookSnapshotPayload::StructuredEntriesV1 {
                        snapshot_entries,
                        entries_total,
                        returned_entries_count,
                        freshness,
                        snapshot_source,
                        stale_reason_code,
                        snapshot_age_ms,
                    } => {
                        let count = snapshot_entries.len();
                        response_map.insert(
                            "snapshot_format".to_string(),
                            json!("structured_entries_v1"),
                        );
                        response_map
                            .insert("snapshot_entries".to_string(), json!(snapshot_entries));
                        response_map.insert("entries_total".to_string(), json!(entries_total));
                        response_map.insert(
                            "returned_entries_count".to_string(),
                            json!(returned_entries_count),
                        );
                        response_map.insert("freshness".to_string(), json!(freshness));
                        response_map.insert("snapshot_source".to_string(), json!(snapshot_source));
                        if let Some(value) = stale_reason_code {
                            response_map
                                .insert("stale_reason_code".to_string(), Value::String(value));
                        }
                        if let Some(value) = snapshot_age_ms {
                            response_map.insert("snapshot_age_ms".to_string(), json!(value));
                        }
                        count
                    }
                };
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.look.success",
                    &json!({
                        "requester_session": response["requester_session"],
                        "target_session": response["target_session"],
                        "snapshot_format": response["snapshot_format"],
                        "snapshot_count": snapshot_count,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.look.relay_error",
                    &json!({
                        "code": error.code.clone(),
                        "message": error.message.clone(),
                        "details": error.details.clone(),
                    }),
                );
                Err(map_relay_error(error))
            }
            Ok(other) => {
                emit_inscription(
                    "mcp.tool.look.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.look.io_error", source)),
        }
    }
}
