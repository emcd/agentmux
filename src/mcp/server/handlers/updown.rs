//! `updown` tool: administer bundle runtime updown. The `tool_updown` method
//! parses the `command` argument and dispatches to `updown_transition`
//! (command="up" or "down").

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::json;

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::validation_tool_error;
use crate::mcp::params::{UPDOWN_COMMAND_DOWN, UPDOWN_COMMAND_UP, UpdownArgs, UpdownParams};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{parse_meta_tool_args, validate_updown_args, validate_updown_params};

#[tool_router(router = tool_router_updown, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "updown",
        description = "Administer the associated bundle's runtime state. Use command=\"up\" to host it or command=\"down\" to unhost it."
    )]
    async fn tool_updown(
        &self,
        Parameters(params): Parameters<UpdownParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_updown_params(&params)?;
        let command = params.command.trim();
        let args = parse_meta_tool_args::<UpdownArgs>(params.args.clone()).map_err(|reason| {
            validation_tool_error(
                "validation_invalid_params",
                "invalid args for updown command",
                Some(json!({
                    "reason": reason,
                    "hint": "pass args as a JSON object; use help query 'updown.up' or 'updown.down' for exact schema",
                })),
            )
        })?;
        match command {
            UPDOWN_COMMAND_UP => {
                validate_updown_args(&args, UPDOWN_COMMAND_UP)?;
                self.updown_transition(UPDOWN_COMMAND_UP, RelayRequest::Up)
            }
            UPDOWN_COMMAND_DOWN => {
                validate_updown_args(&args, UPDOWN_COMMAND_DOWN)?;
                self.updown_transition(UPDOWN_COMMAND_DOWN, RelayRequest::Down)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "updown command must be \"up\" or \"down\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn updown_transition(
        &self,
        command: &str,
        request: RelayRequest,
    ) -> Result<CallToolResult, McpError> {
        let event_prefix = format!("mcp.tool.updown.{command}");
        let request_event = format!("{event_prefix}.request");
        let success_event = format!("{event_prefix}.success");
        let io_error_event = format!("{event_prefix}.io_error");
        emit_inscription(
            request_event.as_str(),
            &json!({
                "namespace": self.associated_namespace(),
                "command": command,
            }),
        );
        match self.request_relay(&request) {
            Ok(RelayResponse::BundleTransition {
                schema_version,
                action,
                bundles,
                changed_bundle_count,
                degraded_bundle_count,
                skipped_bundle_count,
                failed_bundle_count,
                changed_any,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "action": action,
                    "bundles": bundles,
                    "changed_bundle_count": changed_bundle_count,
                    "degraded_bundle_count": degraded_bundle_count,
                    "skipped_bundle_count": skipped_bundle_count,
                    "failed_bundle_count": failed_bundle_count,
                    "changed_any": changed_any,
                });
                emit_inscription(
                    success_event.as_str(),
                    &json!({
                        "namespace": self.associated_namespace(),
                        "action": response["action"],
                        "changed_bundle_count": response["changed_bundle_count"],
                        "degraded_bundle_count": response["degraded_bundle_count"],
                        "skipped_bundle_count": response["skipped_bundle_count"],
                        "failed_bundle_count": response["failed_bundle_count"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response(event_prefix.as_str(), other)),
            Err(source) => Err(self.map_relay_call_error(io_error_event.as_str(), source)),
        }
    }
}
