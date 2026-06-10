//! `raww` tool: write raw text directly to one target session.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::json;

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::{internal_tool_error, map_relay_error, validation_tool_error};
use crate::mcp::params::RawwParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::validate_raww_request;

#[tool_router(router = tool_router_raww, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "raww",
        description = "Write raw text directly to one target session."
    )]
    async fn tool_raww(
        &self,
        Parameters(params): Parameters<RawwParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_raww_request(&params)?;
        emit_inscription(
            "mcp.tool.raww.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "request_id": params.request_id.clone(),
                "target_session": params.target_session.clone(),
                "text_length": params.text.len(),
                "no_enter": params.no_enter,
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

        let request = RelayRequest::Raww {
            request_id: params.request_id.clone(),
            requester_session,
            target_session: params.target_session.clone(),
            text: params.text.clone(),
            no_enter: params.no_enter,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Raww {
                schema_version,
                status,
                target_session,
                transport,
                request_id,
                message_id,
                details,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "status": status,
                    "target_session": target_session,
                    "transport": transport,
                    "request_id": request_id,
                    "message_id": message_id,
                    "details": details,
                });
                emit_inscription(
                    "mcp.tool.raww.success",
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "status": response["status"],
                        "target_session": response["target_session"],
                        "transport": response["transport"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.raww.relay_error",
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
                    "mcp.tool.raww.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.raww.io_error", source)),
        }
    }
}
