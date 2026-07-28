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
                "namespace": self.associated_namespace(),
                "request_id": params.request_id.clone(),
                "targets": params.targets.clone(),
                "broadcast": params.broadcast,
                "message_length": params.message.len(),
            }),
        );
        let requester_session = self.require_associated_sender_session()?;
        // Fill in the namespace the relay now requires on every target. Done
        // after sender resolution so an unassociated server fails as
        // `validation_unassociated_server` regardless of target shape.
        let targets = qualify_send_targets(&params.targets, self.associated_namespace())?;

        let request = RelayRequest::Send {
            request_id: params.request_id.clone(),
            requester_session,
            message: params.message.clone(),
            targets,
            broadcast: params.broadcast,
            quiet_window_ms: None,
            on_behalf_of: None,
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
                // Omit absent optional fields rather than serializing them as
                // null: request_id (present only when the caller provided one) and
                // sender_display_name follow the relay's skip_serializing_if
                // semantics, matching the identity fields below.
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                if let Some(request_id) = request_id {
                    response_map.insert("request_id".to_string(), Value::String(request_id));
                }
                response_map.insert(
                    "requester_session".to_string(),
                    Value::String(requester_session),
                );
                if let Some(sender_display_name) = sender_display_name {
                    response_map.insert(
                        "sender_display_name".to_string(),
                        Value::String(sender_display_name),
                    );
                }
                if let Some(identity) = authenticated_identity {
                    response_map.insert(
                        "authenticated_identity".to_string(),
                        Value::String(identity),
                    );
                }
                if let Some(delegate) = on_behalf_of {
                    response_map.insert("on_behalf_of".to_string(), Value::String(delegate));
                }
                response_map.insert("results".to_string(), json!(results));
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.send.success",
                    &json!({
                        "result_count": response["results"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.send", other)),
            Err(source) => Err(self.map_relay_call_error("mcp.tool.send.io_error", source)),
        }
    }
}
