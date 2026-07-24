//! `change` tool: rotate a principal credential. The `tool_change` method
//! parses the `command` argument and dispatches to `change_psk`
//! (command="psk").

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::{Map, Value, json};

use crate::relay::{RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::errors::validation_tool_error;
use crate::mcp::params::{CHANGE_COMMAND_PSK, ChangeParams, ChangePskArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{
    parse_meta_tool_args, resolve_credential_destination, validate_change_params,
    validate_change_psk_args,
};

#[tool_router(router = tool_router_change, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "change",
        description = "Rotate a principal credential. Use command=\"psk\" to generate a new PSK for an existing principal_id and return it, or write it to an output path or the principal's config (write_to_config)."
    )]
    async fn tool_change(
        &self,
        Parameters(params): Parameters<ChangeParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_change_params(&params)?;
        let command = params.command.trim();
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
        let destination = resolve_credential_destination(
            args.output_path.as_deref(),
            args.write_to_config.unwrap_or(false),
        )?;
        emit_inscription(
            "mcp.tool.change.psk.request",
            &json!({
                "namespace": self.associated_namespace(),
                "principal_id": principal_id,
                "has_output": args.output_path.is_some(),
                "write_to_config": args.write_to_config.unwrap_or(false),
            }),
        );
        let request = RelayRequest::ChangePsk {
            principal_id: principal_id.clone(),
            destination,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::ChangePsk {
                schema_version,
                principal_id,
                psk,
                written_path,
            }) => {
                // Build the payload conditionally: `psk` only in Response mode,
                // `written_path` only when a file was written (per the
                // mcp-tool-surface success-payload contract).
                let mut response = Map::new();
                response.insert("schema_version".to_string(), json!(schema_version));
                response.insert("principal_id".to_string(), json!(principal_id));
                if let Some(psk) = psk {
                    response.insert("psk".to_string(), json!(psk));
                }
                if let Some(written_path) = &written_path {
                    response.insert("written_path".to_string(), json!(written_path));
                }
                emit_inscription(
                    "mcp.tool.change.psk.success",
                    &json!({
                        "principal_id": response["principal_id"],
                        "written": written_path.is_some(),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(
                    Value::Object(response),
                )?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.change.psk", other)),
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.change.psk.io_error", source))
            }
        }
    }
}
