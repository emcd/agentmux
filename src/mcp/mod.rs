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
        let response = json!({
            "schema_version": "1",
            "bundle": self.state.configuration.bundle_paths.bundle_name,
            "sender_session": self.state.configuration.sender_session,
            "sessions": [],
            "note": "Session discovery is not implemented yet."
        });
        Ok(CallToolResult::success(vec![Content::json(response)?]))
    }

    #[tool(description = "Submit a chat message to explicit targets or broadcast.")]
    async fn chat(
        &self,
        Parameters(params): Parameters<ChatParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_chat_request(&params)?;
        let target_count = if params.broadcast {
            0usize
        } else {
            params.targets.len()
        };
        let response = json!({
            "schema_version": "1",
            "accepted": false,
            "bundle": self.state.configuration.bundle_paths.bundle_name,
            "sender_session": self.state.configuration.sender_session,
            "target_mode": if params.broadcast { "broadcast" } else { "targets" },
            "target_count": target_count,
            "note": "Message routing is not implemented yet."
        });
        Ok(CallToolResult::success(vec![Content::json(response)?]))
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
        return Err(McpError::invalid_params("message must be non-empty", None));
    }
    if params.broadcast && !params.targets.is_empty() {
        return Err(McpError::invalid_params(
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if !params.broadcast && params.targets.is_empty() {
        return Err(McpError::invalid_params(
            "provide at least one target or set broadcast=true",
            None,
        ));
    }
    Ok(())
}
