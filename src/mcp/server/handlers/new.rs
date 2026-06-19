//! `new` tool: register a principal credential. The `tool_new` method parses
//! the `command` argument and dispatches to `new_peer` (command="peer").

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
use crate::mcp::params::{NEW_COMMAND_PEER, NewParams, NewPeerArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{parse_meta_tool_args, validate_new_params, validate_new_peer_args};

#[tool_router(router = tool_router_new, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "new",
        description = "Register a principal credential. Use command=\"peer\" to mint a PSK for a principal_id and return it (or write it to an output path)."
    )]
    async fn tool_new(
        &self,
        Parameters(params): Parameters<NewParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_new_params(&params)?;
        let command = params.command.trim();
        match command {
            NEW_COMMAND_PEER => {
                let args = parse_meta_tool_args::<NewPeerArgs>(params.args.clone()).map_err(
                    |reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for new peer command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'new.peer' for exact schema",
                            })),
                        )
                    },
                )?;
                self.new_peer(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "new command must be \"peer\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn new_peer(&self, args: NewPeerArgs) -> Result<CallToolResult, McpError> {
        validate_new_peer_args(&args)?;
        let principal_id = args
            .principal_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "principal_id is required for new peer",
                    None,
                )
            })?
            .to_string();
        let scope = args
            .scope
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let output_path = args
            .output_path
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        emit_inscription(
            "mcp.tool.new.peer.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "principal_id": principal_id,
                "has_output": output_path.is_some(),
            }),
        );
        let request = RelayRequest::NewPeer {
            principal_id: principal_id.clone(),
            scope,
            output_path,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::NewPeer {
                schema_version,
                principal_id,
                principal_type,
                psk,
                output_path,
                config_snippet,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "principal_id": principal_id,
                    "principal_type": principal_type,
                    "psk": psk,
                    "output_path": output_path,
                    "config_snippet": config_snippet,
                });
                emit_inscription(
                    "mcp.tool.new.peer.success",
                    &json!({
                        "principal_id": response["principal_id"],
                        "principal_type": response["principal_type"],
                        "written": output_path.is_some(),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.new.peer.relay_error",
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
                    "mcp.tool.new.peer.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.new.peer.io_error", source)),
        }
    }
}
