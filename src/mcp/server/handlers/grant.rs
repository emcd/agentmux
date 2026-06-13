//! `grant` tool: inspect or resolve pending ACP permission requests. The
//! `tool_grant` method parses the `command` argument and dispatches to
//! `grant_list` (command="list") or `grant_resolve` (command="resolve").

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
use crate::mcp::params::{
    GRANT_COMMAND_LIST, GRANT_COMMAND_RESOLVE, GrantListArgs, GrantParams, GrantResolveArgs,
};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{
    parse_meta_tool_args, validate_grant_list_args, validate_grant_params,
    validate_grant_resolve_args,
};

#[tool_router(router = tool_router_grant, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "grant",
        description = "Inspect or resolve pending ACP permission requests. Use command=\"list\" to enumerate pending requests, command=\"resolve\" to decide one."
    )]
    async fn tool_grant(
        &self,
        Parameters(params): Parameters<GrantParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_grant_params(&params)?;
        let command = params
            .command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "command is required; allowed values are \"list\" or \"resolve\"",
                    None,
                )
            })?;
        match command {
            GRANT_COMMAND_LIST => {
                let args = parse_meta_tool_args::<GrantListArgs>(params.args.clone())
                    .map_err(|reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for grant list command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'grant.list' for exact schema",
                            })),
                        )
                    })?;
                self.grant_list(args)
            }
            GRANT_COMMAND_RESOLVE => {
                let args = parse_meta_tool_args::<GrantResolveArgs>(params.args.clone())
                    .map_err(|reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for grant resolve command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'grant.resolve' for exact schema",
                            })),
                        )
                    })?;
                self.grant_resolve(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "grant command must be \"list\" or \"resolve\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn grant_list(&self, args: GrantListArgs) -> Result<CallToolResult, McpError> {
        validate_grant_list_args(&args)?;
        emit_inscription(
            "mcp.tool.grant.list.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
            }),
        );
        let request = RelayRequest::ChoicesList;
        match self.request_relay(&request) {
            Ok(RelayResponse::ChoicesList {
                schema_version,
                pending_requests,
            }) => {
                let pending_count = pending_requests.len();
                let response = json!({
                    "schema_version": schema_version,
                    "pending_requests": pending_requests,
                });
                emit_inscription(
                    "mcp.tool.grant.list.success",
                    &json!({
                        "pending_count": pending_count,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.grant.list.relay_error",
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
                    "mcp.tool.grant.list.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.grant.list.io_error", source))
            }
        }
    }

    fn grant_resolve(&self, args: GrantResolveArgs) -> Result<CallToolResult, McpError> {
        validate_grant_resolve_args(&args)?;
        let permission_request_id = args
            .permission_request_id
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let outcome = args
            .outcome
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let option_id = args
            .option_id
            .as_ref()
            .map(|value| value.trim().to_string());
        emit_inscription(
            "mcp.tool.grant.resolve.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "permission_request_id": permission_request_id,
                "outcome": outcome,
                "has_option_id": option_id.is_some(),
            }),
        );
        let request = RelayRequest::ChoicesPick {
            choice_request_id: permission_request_id.clone(),
            outcome: outcome.clone(),
            option_id: option_id.clone(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::ChoicesPick {
                schema_version,
                status,
                choice_request_id: permission_request_id,
                outcome,
                reason_code,
                reason,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "status": status,
                    "permission_request_id": permission_request_id,
                    "outcome": outcome,
                    "reason_code": reason_code,
                    "reason": reason,
                });
                emit_inscription(
                    "mcp.tool.grant.resolve.success",
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "permission_request_id": response["permission_request_id"],
                        "status": response["status"],
                        "outcome": response["outcome"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.grant.resolve.relay_error",
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
                    "mcp.tool.grant.resolve.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.grant.resolve.io_error", source))
            }
        }
    }
}
