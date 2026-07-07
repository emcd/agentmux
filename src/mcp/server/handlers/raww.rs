//! `raww` tool: write raw text directly to one target session's input,
//! bypassing normal chat/message semantics.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::{Value, json};

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::validation_tool_error;
use crate::mcp::params::RawwParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::{qualify_target, validate_raww_request};

#[tool_router(router = tool_router_raww, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "raww",
        description = "Write raw text directly to one target session's input, bypassing normal chat/message semantics; use with care. Target is a bare id in the associated bundle or a fully-qualified id@namespace peer."
    )]
    async fn tool_raww(
        &self,
        Parameters(params): Parameters<RawwParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_raww_request(&params)?;
        emit_inscription(
            "mcp.tool.raww.request",
            &json!({
                "namespace": self.associated_namespace(),
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

        // Qualify a bare target to the bound bundle (mirrors send); an
        // already-qualified `@<namespace>` target passes through so cross-bundle
        // writes still work. Done after sender resolution so an unidentified
        // sender fails as `validation_unknown_sender` regardless of target shape.
        let target_session = qualify_target(&params.target_session, self.associated_namespace())?;

        let request = RelayRequest::Raww {
            request_id: params.request_id.clone(),
            requester_session,
            target_session,
            text: params.text.clone(),
            no_enter: params.no_enter,
            on_behalf_of: None,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Raww {
                schema_version,
                status,
                target_session,
                transport,
                request_id,
                message_id,
            }) => {
                // Omit absent optional fields rather than serializing them as
                // null: request_id and message_id follow the relay's
                // skip_serializing_if semantics (the json! re-box would otherwise
                // force them back to null).
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                response_map.insert("status".to_string(), Value::String(status));
                response_map.insert("target_session".to_string(), Value::String(target_session));
                response_map.insert("transport".to_string(), json!(transport));
                if let Some(request_id) = request_id {
                    response_map.insert("request_id".to_string(), Value::String(request_id));
                }
                if let Some(message_id) = message_id {
                    response_map.insert("message_id".to_string(), Value::String(message_id));
                }
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.raww.success",
                    &json!({
                        "namespace": self.associated_namespace(),
                        "status": response["status"],
                        "target_session": response["target_session"],
                        "transport": response["transport"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.raww", other)),
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.raww.io_error", source)),
        }
    }
}
