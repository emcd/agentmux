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

use crate::mcp::errors::{
    UNASSOCIATED_SERVER_REMEDY, internal_tool_error, map_relay_error, map_relay_request_failure,
    startup_fault_error, unassociated_server_error,
};

/// A startup failure retained instead of aborting the process.
///
/// MCP negotiates tool inventory at `initialize`, so failing startup erases the
/// advertised surface rather than degrading it: agents then call tools their
/// context says exist, and some harnesses never recover. A retained fault keeps
/// the surface intact and delivers the cause to the actor who can repair it,
/// rather than to a log nobody reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpStartupFault {
    pub code: String,
    pub message: String,
}

/// Whether the server can serve requests, or is holding a startup fault.
///
/// Relay reachability is deliberately **not** part of this: a complete
/// operational context is `Ready` even when the relay is down, and reachability
/// surfaces per request as `relay_unavailable`.
#[derive(Clone, Debug)]
pub enum McpReadiness {
    Ready,
    Unavailable(McpStartupFault),
}

/// Configuration provided when booting MCP stdio service.
#[derive(Clone, Debug)]
pub struct McpConfiguration {
    pub configuration_root: PathBuf,
    pub state_root: PathBuf,
    pub associated_bundle_paths: Option<BundleRuntimePaths>,
    pub sender_session: Option<String>,
    /// Snapshotted at startup and never recomputed: a fault that made the
    /// process unable to build an operational context does not resolve itself
    /// while the process runs.
    pub readiness: McpReadiness,
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

    /// Map a non-success relay response to the shared MCP error taxonomy,
    /// emitting the matching inscription under `event_prefix`. Collapses the two
    /// failure tails every tool repeats: a relay-side `RelayResponse::Error`
    /// (`{event_prefix}.relay_error`) and any unexpected variant
    /// (`{event_prefix}.unexpected_response`). Callers match the success variant
    /// inline and route every other arm here.
    pub(super) fn map_nonsuccess_relay_response(
        &self,
        event_prefix: &str,
        response: RelayResponse,
    ) -> rmcp::ErrorData {
        match response {
            RelayResponse::Error { error } => {
                emit_inscription(
                    &format!("{event_prefix}.relay_error"),
                    &json!({
                        "code": error.code.clone(),
                        "message": error.message.clone(),
                        "details": error.details.clone(),
                    }),
                );
                map_relay_error(error)
            }
            other => {
                emit_inscription(
                    &format!("{event_prefix}.unexpected_response"),
                    &json!({"response": other}),
                );
                internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                )
            }
        }
    }

    pub(super) fn map_relay_stream_failure(
        &self,
        event: &str,
        source: std::io::Error,
    ) -> rmcp::ErrorData {
        emit_inscription(event, &json!({"error": source.to_string()}));
        if self.is_associated() {
            let relay_paths = RelayRuntimePaths::resolve(&self.state.configuration.state_root);
            return map_relay_request_failure(&relay_paths.relay_socket, source);
        }
        // The stream is absent because the server never became operational, not
        // because a present relay failed; report the retained cause, or the
        // actionable startup contract when there is none.
        self.unavailable_error()
    }

    /// Whether the server holds a live relay stream. True only when both a sender
    /// session and an associated bundle are configured, mirroring the
    /// `relay_stream` construction in `new`; this is the real precondition every
    /// relay-backed tool shares.
    fn is_associated(&self) -> bool {
        self.state.configuration.sender_session.is_some()
            && self.state.configuration.associated_bundle_paths.is_some()
    }

    /// The error a tool returns when the server cannot serve it.
    ///
    /// A retained startup fault names the actual defect, which is what makes it
    /// repairable; without one the server is simply unassociated, and the
    /// actionable contract is the startup command.
    pub(super) fn unavailable_error(&self) -> rmcp::ErrorData {
        match &self.state.configuration.readiness {
            McpReadiness::Ready => unassociated_server_error(),
            McpReadiness::Unavailable(fault) => startup_fault_error(fault),
        }
    }

    /// Guards a relay-backed path that needs only an associated relay stream, not
    /// the sender value: the relay-wide `list` discovery commands. Returns the
    /// canonical unassociated-server error so the failure precedes any relay
    /// contact rather than surfacing later as a stream fault.
    /// Rejects a retained startup fault before a handler does any work.
    ///
    /// A fault can coexist with a complete association: an unwritable
    /// inscriptions sink leaves bundle and session fully resolved, so gating on
    /// association alone would let the fault stay invisible on exactly the
    /// servers that keep working -- the one outcome retaining it exists to
    /// prevent. `help` deliberately does not call this, so an operator can still
    /// read the server's own account of its state.
    pub(super) fn require_ready(&self) -> Result<(), rmcp::ErrorData> {
        match &self.state.configuration.readiness {
            McpReadiness::Ready => Ok(()),
            McpReadiness::Unavailable(fault) => Err(startup_fault_error(fault)),
        }
    }

    pub(super) fn require_association(&self) -> Result<(), rmcp::ErrorData> {
        self.require_ready()?;
        if self.is_associated() {
            Ok(())
        } else {
            Err(self.unavailable_error())
        }
    }

    /// Resolves the sender session value for a handler that needs it (`send`,
    /// `look`, `raww`, local `list.principals`), rejecting an unassociated server
    /// with the same canonical error. Association requires both the sender session
    /// and a bundle, so a partial configuration is unassociated by definition.
    pub(super) fn require_associated_sender_session(&self) -> Result<String, rmcp::ErrorData> {
        self.require_ready()?;
        match (
            self.state.configuration.sender_session.as_ref(),
            self.state.configuration.associated_bundle_paths.as_ref(),
        ) {
            (Some(session), Some(_)) => Ok(session.clone()),
            _ => Err(self.unavailable_error()),
        }
    }

    pub(super) fn associated_namespace(&self) -> Option<&str> {
        self.state
            .configuration
            .associated_bundle_paths
            .as_ref()
            .map(|paths| paths.bundle_name.as_str())
    }

    /// A factual snapshot of the server's association, surfaced in `help` output
    /// so a caller can discover an unassociated server before invoking a
    /// relay-backed tool.
    pub(super) fn association_status(&self) -> serde_json::Value {
        if self.is_associated() {
            json!({
                "associated": true,
                "namespace": self.associated_namespace(),
                "session": self.state.configuration.sender_session,
            })
        } else {
            json!({
                "associated": false,
                "remedy": UNASSOCIATED_SERVER_REMEDY,
            })
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        const BASE: &str = "agentmux MCP server for tmux-backed multi-agent coordination.";
        let instructions = if self.is_associated() {
            format!(
                "{BASE} Associated with session '{session}' in namespace '{namespace}'.",
                session = self
                    .state
                    .configuration
                    .sender_session
                    .as_deref()
                    .unwrap_or_default(),
                namespace = self.associated_namespace().unwrap_or_default(),
            )
        } else {
            format!(
                "{BASE} This server is unassociated (no bundle/session); relay-backed \
                 tools fail with validation_unassociated_server. Start it with \
                 `{UNASSOCIATED_SERVER_REMEDY}`."
            )
        };
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
    }
}

/// Runs the MCP stdio service and blocks until shutdown.
pub async fn run(configuration: McpConfiguration) -> Result<()> {
    let server = McpServer::new(configuration);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
