//! MCP server surface for agentmux.

use std::{path::Path, sync::Arc};

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
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::relay::{ChatDeliveryMode, RelayError, RelayRequest, RelayResponse, request_relay};
use crate::runtime::inscriptions::emit_inscription;
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
struct SendParams {
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    request_id: Option<String>,
    /// Message body to route to targets.
    message: String,
    /// Explicit target recipients by session id or display name (one or many).
    #[serde(default)]
    targets: Vec<String>,
    /// Broadcast to all known sessions for the bundle.
    #[serde(default)]
    broadcast: bool,
    /// Delivery behavior: async queues and returns immediately, sync blocks for completion.
    #[serde(default)]
    delivery_mode: SendDeliveryModeParam,
    /// Optional quiescence timeout override in milliseconds.
    #[serde(default)]
    quiescence_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookParams {
    /// Session identifier to inspect.
    target_session: String,
    /// Optional override for bundle name (MVP rejects cross-bundle requests).
    #[serde(default)]
    bundle_name: Option<String>,
    /// Optional number of pane snapshot lines to capture.
    #[serde(default)]
    lines: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SendDeliveryModeParam {
    #[default]
    Async,
    Sync,
}

impl From<SendDeliveryModeParam> for ChatDeliveryMode {
    fn from(value: SendDeliveryModeParam) -> Self {
        match value {
            SendDeliveryModeParam::Async => ChatDeliveryMode::Async,
            SendDeliveryModeParam::Sync => ChatDeliveryMode::Sync,
        }
    }
}

const MIN_LOOK_LINES: u64 = 1;
const MAX_LOOK_LINES: u64 = 1000;

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
        emit_inscription(
            "mcp.tool.list.request",
            &json!({
                "bundle_name": self.state.configuration.bundle_paths.bundle_name,
                "sender_session": self.state.configuration.sender_session.clone(),
            }),
        );
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
                emit_inscription(
                    "mcp.tool.list.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
                        "recipient_count": response["recipients"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.list.relay_error",
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
                    "mcp.tool.list.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                emit_inscription(
                    "mcp.tool.list.io_error",
                    &json!({"error": source.to_string()}),
                );
                Err(map_relay_request_failure(
                    &self.state.configuration.bundle_paths.relay_socket,
                    source,
                ))
            }
        }
    }

    #[tool(description = "Submit a message to explicit targets or broadcast.")]
    async fn send(
        &self,
        Parameters(params): Parameters<SendParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_send_request(&params)?;
        emit_inscription(
            "mcp.tool.send.request",
            &json!({
                "bundle_name": self.state.configuration.bundle_paths.bundle_name,
                "request_id": params.request_id.clone(),
                "targets": params.targets.clone(),
                "broadcast": params.broadcast,
                "delivery_mode": params.delivery_mode,
                "quiescence_timeout_ms": params.quiescence_timeout_ms,
                "message_length": params.message.len(),
            }),
        );
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
            delivery_mode: params.delivery_mode.into(),
            quiet_window_ms: None,
            quiescence_timeout_ms: params.quiescence_timeout_ms,
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
                delivery_mode,
                status,
                results,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "request_id": request_id,
                    "sender_session": sender_session,
                    "sender_display_name": sender_display_name,
                    "delivery_mode": delivery_mode,
                    "status": status,
                    "results": results,
                });
                emit_inscription(
                    "mcp.tool.send.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
                        "status": response["status"],
                        "result_count": response["results"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.send.relay_error",
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
                    "mcp.tool.send.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                emit_inscription(
                    "mcp.tool.send.io_error",
                    &json!({"error": source.to_string()}),
                );
                Err(map_relay_request_failure(
                    &self.state.configuration.bundle_paths.relay_socket,
                    source,
                ))
            }
        }
    }

    #[tool(description = "Inspect a target session pane snapshot for this bundle.")]
    async fn look(
        &self,
        Parameters(params): Parameters<LookParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_look_request(&params)?;
        emit_inscription(
            "mcp.tool.look.request",
            &json!({
                "bundle_name": self.state.configuration.bundle_paths.bundle_name,
                "requester_session": self.state.configuration.sender_session.clone(),
                "target_session": params.target_session.clone(),
                "requested_bundle_name": params.bundle_name.clone(),
                "lines": params.lines,
            }),
        );
        let requester_session = self
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

        let request = RelayRequest::Look {
            requester_session,
            target_session: params.target_session.clone(),
            lines: params.lines.map(|value| value as usize),
            bundle_name: params.bundle_name.clone(),
        };
        match request_relay(
            &self.state.configuration.bundle_paths.relay_socket,
            &request,
        ) {
            Ok(RelayResponse::Look {
                schema_version,
                bundle_name,
                requester_session,
                target_session,
                captured_at,
                snapshot_lines,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "requester_session": requester_session,
                    "target_session": target_session,
                    "captured_at": captured_at,
                    "snapshot_lines": snapshot_lines,
                });
                emit_inscription(
                    "mcp.tool.look.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
                        "requester_session": response["requester_session"],
                        "target_session": response["target_session"],
                        "snapshot_line_count": response["snapshot_lines"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.look.relay_error",
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
                    "mcp.tool.look.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                emit_inscription(
                    "mcp.tool.look.io_error",
                    &json!({"error": source.to_string()}),
                );
                Err(map_relay_request_failure(
                    &self.state.configuration.bundle_paths.relay_socket,
                    source,
                ))
            }
        }
    }
}

#[tool_handler]
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

fn validate_send_request(params: &SendParams) -> Result<(), McpError> {
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
    if matches!(params.quiescence_timeout_ms, Some(0)) {
        return Err(validation_tool_error(
            "validation_invalid_quiescence_timeout",
            "quiescence_timeout_ms must be greater than zero milliseconds",
            None,
        ));
    }
    Ok(())
}

fn validate_look_request(params: &LookParams) -> Result<(), McpError> {
    if params.target_session.trim().is_empty() {
        return Err(validation_tool_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }

    if let Some(lines) = params.lines
        && !(MIN_LOOK_LINES..=MAX_LOOK_LINES).contains(&lines)
    {
        return Err(validation_tool_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": lines,
                "min": MIN_LOOK_LINES,
                "max": MAX_LOOK_LINES,
            })),
        ));
    }
    Ok(())
}

fn map_relay_error(error: RelayError) -> McpError {
    if error.code.starts_with("validation_") || error.code == "authorization_forbidden" {
        return validation_tool_error(&error.code, &error.message, error.details);
    }
    internal_tool_error(&error.code, &error.message, error.details)
}

fn map_relay_request_failure(socket_path: &Path, source: std::io::Error) -> McpError {
    if is_relay_unavailable_error(&source) {
        return internal_tool_error(
            "relay_unavailable",
            "relay is unavailable; start agentmux host relay <bundle-id> with matching state-directory",
            Some(json!({
                "relay_socket": socket_path,
                "io_error_kind": format!("{:?}", source.kind()),
                "cause": source.to_string(),
            })),
        );
    }

    internal_tool_error(
        "internal_unexpected_failure",
        "relay request failed",
        Some(json!({
            "relay_socket": socket_path,
            "io_error_kind": format!("{:?}", source.kind()),
            "cause": source.to_string(),
        })),
    )
}

fn is_relay_unavailable_error(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::UnexpectedEof
    )
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
