//! `change` tool: rotate a principal credential. The `tool_change` method
//! parses the `command` argument and dispatches to `change_psk`
//! (command="psk").

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
use crate::mcp::params::{CHANGE_COMMAND_PSK, ChangeParams, ChangePskArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{
    parse_meta_tool_args, validate_change_params, validate_change_psk_args,
};

#[tool_router(router = tool_router_change, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "change",
        description = "Rotate a principal credential. Use command=\"psk\" to generate a new PSK for an existing principal_id and return it."
    )]
    async fn tool_change(
        &self,
        Parameters(params): Parameters<ChangeParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_change_params(&params)?;
        let command = params
            .command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "command is required; allowed value is \"psk\"",
                    None,
                )
            })?;
        match command {
            CHANGE_COMMAND_PSK => {
                let args = parse_meta_tool_args::<ChangePskArgs>(params.args.clone()).map_err(
                    |reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for change psk command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'change.psk' for exact schema",
                            })),
                        )
                    },
                )?;
                self.change_psk(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "change command must be \"psk\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn change_psk(&self, args: ChangePskArgs) -> Result<CallToolResult, McpError> {
        validate_change_psk_args(&args)?;
        let principal_id = args
            .principal_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "principal_id is required for change psk",
                    None,
                )
            })?
            .to_string();
        emit_inscription(
            "mcp.tool.change.psk.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "principal_id": principal_id,
            }),
        );
        let request = RelayRequest::ChangePsk {
            principal_id: principal_id.clone(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::ChangePsk {
                schema_version,
                principal_id,
                psk,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "principal_id": principal_id,
                    "psk": psk,
                });
                emit_inscription(
                    "mcp.tool.change.psk.success",
                    &json!({ "principal_id": response["principal_id"] }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.change.psk.relay_error",
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
                    "mcp.tool.change.psk.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.change.psk.io_error", source))
            }
        }
    }
}
