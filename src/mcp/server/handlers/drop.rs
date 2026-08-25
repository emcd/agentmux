//! `drop` tool: delete a principal from the relay-wide store. The `tool_drop`
//! method parses the `command` argument and dispatches to `drop_peer`
//! (command="peer").

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
use crate::mcp::params::{DROP_COMMAND_PEER, DropParams, DropPeerArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{parse_meta_tool_args, validate_drop_params, validate_drop_peer_args};

#[tool_router(router = tool_router_drop, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "drop",
        description = "Delete a principal credential. Use command=\"peer\" to remove an existing principal_id from the relay principal store; its credential stops authenticating and any session bound to it is disconnected. Credential files on disk are left in place."
    )]
    async fn tool_drop(
        &self,
        Parameters(params): Parameters<DropParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_drop_params(&params)?;
        let command = params.command.trim();
        match command {
            DROP_COMMAND_PEER => {
                let args =
                    parse_meta_tool_args::<DropPeerArgs>(params.args.clone()).map_err(|reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for drop peer command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'drop.peer' for exact schema",
                            })),
                        )
                    })?;
                self.drop_peer(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "drop command must be \"peer\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn drop_peer(&self, args: DropPeerArgs) -> Result<CallToolResult, McpError> {
        validate_drop_peer_args(&args)?;
        let principal_id = args
            .principal_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "principal_id is required for drop peer",
                    None,
                )
            })?
            .to_string();
        emit_inscription(
            "mcp.tool.drop.peer.request",
            &json!({
                "namespace": self.associated_namespace(),
                "principal_id": principal_id,
            }),
        );
        let request = RelayRequest::DropPeer {
            principal_id: principal_id.clone(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::DropPeer {
                schema_version,
                principal_id,
                principal_type,
                credential_path,
            }) => {
                // `credential_path` appears only where the relay owns a canonical
                // location, which is session principals alone (per the
                // mcp-tool-surface success-payload contract).
                let mut response = Map::new();
                response.insert("schema_version".to_string(), json!(schema_version));
                response.insert("principal_id".to_string(), json!(principal_id));
                response.insert("principal_type".to_string(), json!(principal_type));
                if let Some(credential_path) = &credential_path {
                    response.insert("credential_path".to_string(), json!(credential_path));
                }
                emit_inscription(
                    "mcp.tool.drop.peer.success",
                    &json!({
                        "principal_id": response["principal_id"],
                        "principal_type": response["principal_type"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(
                    Value::Object(response),
                )?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.drop.peer", other)),
            Err(source) => Err(self.map_relay_call_error("mcp.tool.drop.peer.io_error", source)),
        }
    }
}
