//! `new` tool: register a principal credential. The `tool_new` method parses
//! the `command` argument and dispatches to `new_peer` (command="peer").

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
use crate::mcp::params::{NEW_COMMAND_PEER, NewParams, NewPeerArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{
    parse_meta_tool_args, resolve_credential_destination, validate_new_params,
    validate_new_peer_args,
};

#[tool_router(router = tool_router_new, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "new",
        description = "Register a principal credential. Use command=\"peer\" to mint a PSK for a principal_id and return it, or write it to an output path or the principal's config (write_to_config)."
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
        let destination = resolve_credential_destination(
            args.output_path.as_deref(),
            args.write_to_config.unwrap_or(false),
        )?;
        // After every validation for the selected command and before any
        // inscription or relay work: a malformed request reports its own
        // fault even while a startup fault is retained.
        self.require_ready()?;
        emit_inscription(
            "mcp.tool.new.peer.request",
            &json!({
                "namespace": self.associated_namespace(),
                "principal_id": principal_id,
                "has_output": args.output_path.is_some(),
                "write_to_config": args.write_to_config.unwrap_or(false),
            }),
        );
        let request = RelayRequest::NewPeer {
            principal_id: principal_id.clone(),
            scope,
            destination,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::NewPeer {
                schema_version,
                principal_id,
                principal_type,
                psk,
                written_path,
                config_snippet,
            }) => {
                // Build the payload conditionally so each optional field appears
                // only for its destination: `psk` only in Response mode,
                // `written_path` only when a file was written (per the
                // mcp-tool-surface success-payload contract).
                let mut response = Map::new();
                response.insert("schema_version".to_string(), json!(schema_version));
                response.insert("principal_id".to_string(), json!(principal_id));
                response.insert("principal_type".to_string(), json!(principal_type));
                response.insert("config_snippet".to_string(), json!(config_snippet));
                if let Some(psk) = psk {
                    response.insert("psk".to_string(), json!(psk));
                }
                if let Some(written_path) = &written_path {
                    response.insert("written_path".to_string(), json!(written_path));
                }
                emit_inscription(
                    "mcp.tool.new.peer.success",
                    &json!({
                        "principal_id": response["principal_id"],
                        "principal_type": response["principal_type"],
                        "written": written_path.is_some(),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(
                    Value::Object(response),
                )?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.new.peer", other)),
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.new.peer.io_error", source)),
        }
    }
}
