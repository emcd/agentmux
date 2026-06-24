//! MCP server service: state types, shared relay helpers, the `#[tool_handler]`
//! impl, and `pub async fn run`. The per-tool `#[tool_router]` impl blocks
//! live in `handlers/`.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use rmcp::{
    ServiceExt,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo},
    tool_handler,
    transport::stdio,
};
use serde_json::json;

use crate::relay::{RelayRequest, RelayResponse, RelayStreamSession};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{BundleRuntimePaths, RelayRuntimePaths};

use crate::mcp::errors::{internal_tool_error, map_relay_request_failure};

/// Configuration provided when booting MCP stdio service.
#[derive(Clone, Debug)]
pub struct McpConfiguration {
    pub configuration_root: PathBuf,
    pub state_root: PathBuf,
    pub associated_bundle_paths: Option<BundleRuntimePaths>,
    pub sender_session: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct McpServer {
    pub(super) state: Arc<McpState>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug)]
pub(super) struct McpState {
    pub(super) configuration: McpConfiguration,
    relay_stream: Mutex<Option<RelayStreamSession>>,
}

impl McpServer {
    pub(crate) fn new(configuration: McpConfiguration) -> Self {
        let relay_paths = RelayRuntimePaths::resolve(&configuration.state_root);
        let relay_stream = configuration
            .sender_session
            .as_ref()
            .zip(configuration.associated_bundle_paths.as_ref())
            .map(|(sender_session, bundle_paths)| {
                RelayStreamSession::new(
                    relay_paths.relay_socket.clone(),
                    bundle_paths.bundle_name.clone(),
                    sender_session.clone(),
                )
            });
        let tool_router = Self::tool_router_list()
            + Self::tool_router_help()
            + Self::tool_router_send()
            + Self::tool_router_look()
            + Self::tool_router_raww()
            + Self::tool_router_choose()
            + Self::tool_router_updown()
            + Self::tool_router_new()
            + Self::tool_router_change();
        Self {
            state: Arc::new(McpState {
                configuration,
                relay_stream: Mutex::new(relay_stream),
            }),
            tool_router,
        }
    }

    pub(super) fn request_relay(
        &self,
        request: &RelayRequest,
    ) -> Result<RelayResponse, std::io::Error> {
        self.request_relay_with_namespace(request, None)
    }

    pub(super) fn request_relay_with_namespace(
        &self,
        request: &RelayRequest,
        namespace: Option<&str>,
    ) -> Result<RelayResponse, std::io::Error> {
        let mut guard = self
            .state
            .relay_stream
            .lock()
            .map_err(|_| std::io::Error::other("failed to lock MCP relay stream session"))?;
        let stream_session = guard.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sender session is not configured for MCP relay stream",
            )
        })?;
        let (response, events) =
            stream_session.request_with_namespace_and_events(request, namespace)?;
        if !events.is_empty() {
            emit_inscription(
                "mcp.tool.stream.events_ignored",
                &json!({
                    "namespace": self.associated_namespace(),
                    "count": events.len(),
                }),
            );
        }
        Ok(response)
    }

    pub(super) fn map_relay_stream_failure(
        &self,
        event: &str,
        source: std::io::Error,
    ) -> rmcp::ErrorData {
        emit_inscription(event, &json!({"error": source.to_string()}));
        if self.state.configuration.associated_bundle_paths.is_some() {
            let relay_paths = RelayRuntimePaths::resolve(&self.state.configuration.state_root);
            return map_relay_request_failure(&relay_paths.relay_socket, source);
        }
        internal_tool_error(
            "internal_unexpected_failure",
            "relay stream is unavailable for unassociated MCP server",
            Some(json!({
                "cause": source.to_string(),
            })),
        )
    }

    pub(super) fn associated_namespace(&self) -> Option<&str> {
        self.state
            .configuration
            .associated_bundle_paths
            .as_ref()
            .map(|paths| paths.bundle_name.as_str())
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("agentmux MCP server for tmux-backed multi-agent coordination.")
    }
}

/// Runs the MCP stdio service and blocks until shutdown.
pub async fn run(configuration: McpConfiguration) -> Result<()> {
    let server = McpServer::new(configuration);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
