//! MCP server surface for tmuxmux.

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, ServiceExt,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::relay::{RelayError, RelayRequest, RelayResponse, request_relay};
use crate::runtime::paths::BundleRuntimePaths;

/// Configuration provided when booting MCP stdio service.
#[derive(Clone, Debug)]
pub struct McpConfiguration {
    pub bundle_paths: BundleRuntimePaths,
    pub sender_session: Option<String>,
}

#[derive(Clone, Debug)]
struct McpServer {
    state: Arc<McpState>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug)]
struct McpState {
    configuration: McpConfiguration,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct ListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatParams {
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    request_id: Option<String>,
    /// Message body to route to targets.
    message: String,
    /// Explicit target sessions (one or many).
    #[serde(default)]
    targets: Vec<String>,
    /// Broadcast to all known sessions for the bundle.
    #[serde(default)]
    broadcast: bool,
}

#[tool_router]
impl McpServer {
    fn new(configuration: McpConfiguration) -> Self {
        Self {
            state: Arc::new(McpState { configuration }),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List potential recipient sessions for this bundle.")]
    async fn list(
        &self,
        Parameters(_params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        match request_relay(
            &self.state.configuration.bundle_paths.relay_socket,
            &RelayRequest::List {
                sender_session: self.state.configuration.sender_session.clone(),
            },
        ) {
            Ok(RelayResponse::List {
                schema_version,
                bundle_name,
                recipients,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "recipients": recipients,
                });
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => Err(map_relay_error(error)),
            Ok(other) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({"response": other})),
            )),
            Err(source) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay request failed",
                Some(json!({"cause": source.to_string()})),
            )),
        }
    }

    #[tool(description = "Submit a chat message to explicit targets or broadcast.")]
    async fn chat(
        &self,
        Parameters(params): Parameters<ChatParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_chat_request(&params)?;
        let sender_session = self
            .state
            .configuration
            .sender_session
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_unknown_sender",
                    "sender session is not configured for this MCP server",
                    None,
                )
            })?;

        let request = RelayRequest::Chat {
            request_id: params.request_id.clone(),
            sender_session,
            message: params.message.clone(),
            targets: params.targets.clone(),
            broadcast: params.broadcast,
        };
        match request_relay(
            &self.state.configuration.bundle_paths.relay_socket,
            &request,
        ) {
            Ok(RelayResponse::Chat {
                schema_version,
                bundle_name,
                request_id,
                sender_session,
                sender_display_name,
                status,
                results,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "request_id": request_id,
                    "sender_session": sender_session,
                    "sender_display_name": sender_display_name,
                    "status": status,
                    "results": results,
                });
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => Err(map_relay_error(error)),
            Ok(other) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({"response": other})),
            )),
            Err(source) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay request failed",
                Some(json!({"cause": source.to_string()})),
            )),
        }
    }
}

#[tool_handler]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "tmuxmux MCP server for tmux-backed multi-agent coordination.".to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Runs the MCP stdio service and blocks until shutdown.
pub async fn run(configuration: McpConfiguration) -> Result<()> {
    let server = McpServer::new(configuration);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn validate_chat_request(params: &ChatParams) -> Result<(), McpError> {
    let message = params.message.trim();
    if message.is_empty() {
        return Err(validation_tool_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if params.broadcast && !params.targets.is_empty() {
        return Err(validation_tool_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if !params.broadcast && params.targets.is_empty() {
        return Err(validation_tool_error(
            "validation_empty_targets",
            "provide at least one target or set broadcast=true",
            None,
        ));
    }
    Ok(())
}

fn map_relay_error(error: RelayError) -> McpError {
    if error.code.starts_with("validation_") {
        return validation_tool_error(&error.code, &error.message, error.details);
    }
    internal_tool_error(&error.code, &error.message, error.details)
}

fn validation_tool_error(
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> McpError {
    McpError::invalid_params(
        message.to_string(),
        Some(error_payload(code, message, details)),
    )
}

fn internal_tool_error(code: &str, message: &str, details: Option<serde_json::Value>) -> McpError {
    McpError::internal_error(
        message.to_string(),
        Some(error_payload(code, message, details)),
    )
}

fn error_payload(
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "code": code,
        "message": message,
        "details": details,
    })
}
