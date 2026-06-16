//! `send` tool: submit a message to explicit targets or broadcast.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::{Value, json};

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::{internal_tool_error, map_relay_error, validation_tool_error};
use crate::mcp::params::SendParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::{qualify_send_targets, validate_send_request};

#[tool_router(router = tool_router_send, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "send",
        description = "Submit a message to explicit targets or broadcast."
    )]
    async fn tool_send(
        &self,
        Parameters(params): Parameters<SendParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_send_request(&params)?;
        emit_inscription(
            "mcp.tool.send.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "request_id": params.request_id.clone(),
                "targets": params.targets.clone(),
                "broadcast": params.broadcast,
                "message_length": params.message.len(),
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
        // Fill in the namespace the relay now requires on every target. Done
        // after sender resolution so an unidentified sender fails as
        // `validation_unknown_sender` regardless of target shape.
        let targets = qualify_send_targets(&params.targets, self.associated_bundle_name())?;

        let request = RelayRequest::Send {
            request_id: params.request_id.clone(),
            requester_session,
            message: params.message.clone(),
            targets,
            broadcast: params.broadcast,
            quiet_window_ms: None,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Send {
                schema_version,
                request_id,
                requester_session,
                sender_display_name,
                authenticated_identity,
                on_behalf_of,
                results,
            }) => {
                let mut response = json!({
                    "schema_version": schema_version,
                    "request_id": request_id,
                    "requester_session": requester_session,
                    "sender_display_name": sender_display_name,
                    "results": results,
                });
                if let Some(identity) = authenticated_identity {
                    response["authenticated_identity"] = Value::String(identity);
                }
                if let Some(delegate) = on_behalf_of {
                    response["on_behalf_of"] = Value::String(delegate);
                }
                emit_inscription(
                    "mcp.tool.send.success",
                    &json!({
                        "result_count": response["results"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.send.relay_error",
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
                    "mcp.tool.send.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.send.io_error", source)),
        }
    }
}
