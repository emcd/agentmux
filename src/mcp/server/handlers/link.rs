//! `link` tool: install a peer credential into the connected relay's
//! relay-owned slot. The `tool_link` method parses the `command` argument
//! and dispatches to `link_peer` (command="peer").
//!
//! The raw PSK travels as an explicit secret argument in request memory
//! only. It is never written to inscriptions, logs, or diagnostics, and no
//! response carries it: the relay omits it structurally.

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
use crate::mcp::params::{LINK_COMMAND_PEER, LinkParams, LinkPeerArgs};
use crate::mcp::server::McpServer;
use crate::mcp::validation::{parse_meta_tool_args, validate_link_params, validate_link_peer_args};

#[tool_router(router = tool_router_link, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "link",
        description = "Install a peer credential into the connected relay's relay-owned slot. Use command=\"peer\" with the local alias and the PSK the opposite relay issued; the PSK travels in request memory only and is never returned, logged, or persisted."
    )]
    async fn tool_link(
        &self,
        Parameters(params): Parameters<LinkParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_link_params(&params)?;
        let command = params.command.trim();
        match command {
            LINK_COMMAND_PEER => {
                let args = parse_meta_tool_args::<LinkPeerArgs>(params.args.clone()).map_err(
                    |reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for link peer command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'link.peer' for exact schema",
                            })),
                        )
                    },
                )?;
                self.link_peer(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "link command must be \"peer\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn link_peer(&self, args: LinkPeerArgs) -> Result<CallToolResult, McpError> {
        validate_link_peer_args(&args)?;
        let alias = args
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "alias is required for link peer",
                    None,
                )
            })?
            .to_string();
        let psk = args
            .psk
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "psk is required for link peer",
                    None,
                )
            })?
            .to_string();
        // The inscription names the alias only: the secret argument must not
        // reach any persisted or logged surface.
        emit_inscription(
            "mcp.tool.link.peer.request",
            &json!({
                "namespace": self.associated_namespace(),
                "alias": alias,
            }),
        );
        let request = RelayRequest::InstallPeerCredential {
            alias: alias.clone(),
            psk,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::InstallPeerCredential {
                schema_version,
                alias,
                written_path,
            }) => {
                let mut response = Map::new();
                response.insert("schema_version".to_string(), json!(schema_version));
                response.insert("alias".to_string(), json!(alias));
                response.insert("written_path".to_string(), json!(written_path));
                emit_inscription(
                    "mcp.tool.link.peer.success",
                    &json!({
                        "alias": response["alias"],
                        "written": true,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(
                    Value::Object(response),
                )?]))
            }
            Ok(other) => Err(map_unexpected_link_response(&alias, other)),
            Err(source) => Err(self.map_relay_call_error("mcp.tool.link.peer.io_error", source)),
        }
    }
}

/// Maps a non-success relay response without inscribing or returning the
/// response payload: an unexpected `NewPeer`/`ChangePsk` variant can carry
/// a PSK, so the shared mapper's full-payload inscription and details are
/// unsafe for this tool. Only the alias (already caller-supplied) and the
/// outcome travel to observability and the caller.
fn map_unexpected_link_response(alias: &str, response: RelayResponse) -> McpError {
    match response {
        RelayResponse::Error { error } => {
            emit_inscription(
                "mcp.tool.link.peer.relay_error",
                &json!({
                    "alias": alias,
                    "code": error.code.clone(),
                }),
            );
            validation_tool_error(error.code.as_str(), error.message.as_str(), None)
        }
        _ => {
            emit_inscription(
                "mcp.tool.link.peer.unexpected_response",
                &json!({"alias": alias}),
            );
            validation_tool_error(
                "internal_unexpected_failure",
                "relay returned an unexpected response for link peer",
                None,
            )
        }
    }
}
