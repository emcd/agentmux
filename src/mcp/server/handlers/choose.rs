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
use serde_json::{Value, json};

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

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
                "namespace": self.associated_namespace(),
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
                decided_by,
                reason_code,
                reason,
            }) => {
                // Omit absent optional fields rather than serializing them as
                // null: decided_by, reason_code, and reason follow the relay's
                // skip_serializing_if semantics (the json! re-box would otherwise
                // force them back to null).
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                response_map.insert("status".to_string(), Value::String(status));
                response_map.insert(
                    "choice_request_id".to_string(),
                    Value::String(choice_request_id),
                );
                response_map.insert("outcome".to_string(), Value::String(outcome));
                if let Some(decided_by) = decided_by {
                    response_map.insert("decided_by".to_string(), Value::String(decided_by));
                }
                if let Some(reason_code) = reason_code {
                    response_map.insert("reason_code".to_string(), Value::String(reason_code));
                }
                if let Some(reason) = reason {
                    response_map.insert("reason".to_string(), Value::String(reason));
                }
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.choose.success",
                    &json!({
                        "namespace": self.associated_namespace(),
                        "choice_request_id": response["choice_request_id"],
                        "status": response["status"],
                        "outcome": response["outcome"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.choose", other)),
            Err(source) => Err(self.map_relay_call_error("mcp.tool.choose.io_error", source)),
        }
    }
}
