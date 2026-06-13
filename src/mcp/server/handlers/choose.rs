//! `choose` tool: submit an ACP-native decision on a pending choice request.
//! Maps to `RelayRequest::ChoicesPick`. The decision actor is association-
//! derived by the relay; caller-supplied sender-like identity fields are
//! rejected as unknown parameters.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::json;

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::{internal_tool_error, map_relay_error};
use crate::mcp::params::ChooseParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::validate_choose_request;

#[tool_router(router = tool_router_choose, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "choose",
        description = "Submit an ACP-native decision on a pending choice request. Set outcome=\"selected\" with an option_id, or outcome=\"cancelled\"."
    )]
    async fn tool_choose(
        &self,
        Parameters(params): Parameters<ChooseParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_choose_request(&params)?;
        let choice_request_id = params
            .choice_request_id
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let outcome = params
            .outcome
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let option_id = params
            .option_id
            .as_ref()
            .map(|value| value.trim().to_string());
        emit_inscription(
            "mcp.tool.choose.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "choice_request_id": choice_request_id,
                "outcome": outcome,
                "has_option_id": option_id.is_some(),
            }),
        );
        let request = RelayRequest::ChoicesPick {
            choice_request_id: choice_request_id.clone(),
            outcome: outcome.clone(),
            option_id: option_id.clone(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::ChoicesPick {
                schema_version,
                status,
                choice_request_id,
                outcome,
                reason_code,
                reason,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "status": status,
                    "choice_request_id": choice_request_id,
                    "outcome": outcome,
                    "reason_code": reason_code,
                    "reason": reason,
                });
                emit_inscription(
                    "mcp.tool.choose.success",
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "choice_request_id": response["choice_request_id"],
                        "status": response["status"],
                        "outcome": response["outcome"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.choose.relay_error",
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
                    "mcp.tool.choose.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.choose.io_error", source)),
        }
    }
}
