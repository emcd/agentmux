//! MCP server surface for agentmux.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

use crate::configuration::{
    BundleConfiguration, ConfigurationError, TargetConfiguration, load_bundle_configuration,
    load_bundle_group_memberships,
};
use crate::relay::{
    ChatDeliveryMode, ListedBundle, ListedBundleState, ListedSession, ListedSessionTransport,
    RelayError, RelayRequest, RelayResponse, RelayStreamClientClass, RelayStreamSession,
    request_relay,
};
use crate::runtime::error::RuntimeError;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::BundleRuntimePaths;

/// Configuration provided when booting MCP stdio service.
#[derive(Clone, Debug)]
pub struct McpConfiguration {
    pub configuration_root: PathBuf,
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
    relay_stream: Mutex<Option<RelayStreamSession>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct ListParams {
    /// List command selector. MVP requires command="sessions".
    #[serde(default)]
    command: Option<String>,
    /// Command-scoped arguments.
    #[serde(default)]
    args: ListArgs,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct ListArgs {
    /// Optional bundle selector. Mutually exclusive with all=true.
    #[serde(default)]
    bundle_name: Option<String>,
    /// Optional all-bundles fanout selector.
    #[serde(default)]
    all: bool,
}

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
    /// Optional ACP turn timeout override in milliseconds.
    #[serde(default)]
    acp_turn_timeout_ms: Option<u64>,
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

const LOOK_LINES_MIN: u64 = 1;
const LOOK_LINES_MAX: u64 = 1000;
const LIST_SESSIONS_SCHEMA_VERSION: &str = "1";
const LIST_COMMAND_SESSIONS: &str = "sessions";

#[tool_router]
impl McpServer {
    fn new(configuration: McpConfiguration) -> Self {
        let relay_stream = configuration.sender_session.as_ref().map(|sender_session| {
            RelayStreamSession::new(
                configuration.bundle_paths.relay_socket.clone(),
                configuration.bundle_paths.bundle_name.clone(),
                sender_session.clone(),
                RelayStreamClientClass::Agent,
            )
        });
        Self {
            state: Arc::new(McpState {
                configuration,
                relay_stream: Mutex::new(relay_stream),
            }),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List sessions for one bundle or fan out across bundles.")]
    async fn list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_list_request(&params)?;
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
        let selected_bundle = params
            .args
            .bundle_name
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        emit_inscription(
            "mcp.tool.list.request",
            &json!({
                "sender_session": sender_session,
                "command": LIST_COMMAND_SESSIONS,
                "bundle_name": selected_bundle,
                "all": params.args.all,
            }),
        );
        if params.args.all {
            let bundles = self.list_sessions_all_bundles(sender_session.as_str())?;
            let response = json!({
                "schema_version": LIST_SESSIONS_SCHEMA_VERSION,
                "bundles": bundles,
            });
            emit_inscription(
                "mcp.tool.list.success",
                &json!({
                    "all": true,
                    "bundle_count": response["bundles"].as_array().map_or(0, |value| value.len()),
                }),
            );
            return Ok(CallToolResult::success(vec![Content::json(response)?]));
        }
        let bundle_name = selected_bundle
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.home_bundle_name().to_string());
        match self.list_sessions_single_bundle(bundle_name.as_str(), sender_session.as_str()) {
            Ok(bundle) => {
                let response = json!({
                    "schema_version": LIST_SESSIONS_SCHEMA_VERSION,
                    "bundle": bundle,
                });
                emit_inscription(
                    "mcp.tool.list.success",
                    &json!({
                        "all": false,
                        "bundle_name": response["bundle"]["id"],
                        "session_count": response["bundle"]["sessions"].as_array().map_or(0, |value| value.len()),
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Err(error) => Err(error),
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
                "acp_turn_timeout_ms": params.acp_turn_timeout_ms,
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
            acp_turn_timeout_ms: params.acp_turn_timeout_ms,
        };
        match self.request_relay(&request) {
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
        match self.request_relay(&request) {
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

    fn list_sessions_single_bundle(
        &self,
        bundle_name: &str,
        sender_session: &str,
    ) -> Result<ListedBundle, McpError> {
        let bundle_paths = BundleRuntimePaths::resolve(
            &self.state.configuration.bundle_paths.state_root,
            bundle_name,
        )
        .map_err(map_runtime_error)?;
        let bundle =
            load_bundle_configuration(&self.state.configuration.configuration_root, bundle_name)
                .map_err(map_configuration_error)?;
        let relay_socket = bundle_paths.relay_socket;
        match request_relay(
            &relay_socket,
            &RelayRequest::List {
                sender_session: Some(sender_session.to_string()),
            },
        ) {
            Ok(RelayResponse::List { bundle, .. }) => Ok(bundle),
            Ok(RelayResponse::Error { error }) => Err(map_relay_error(error)),
            Ok(other) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({"response": other})),
            )),
            Err(source)
                if is_relay_unavailable_error(&source)
                    && bundle_name == self.home_bundle_name() =>
            {
                Ok(self.synthesize_down_bundle(&bundle, &relay_socket))
            }
            Err(source) => Err(map_relay_request_failure(&relay_socket, source)),
        }
    }

    fn list_sessions_all_bundles(
        &self,
        sender_session: &str,
    ) -> Result<Vec<ListedBundle>, McpError> {
        let memberships =
            load_bundle_group_memberships(&self.state.configuration.configuration_root)
                .map_err(map_configuration_error)?;
        let mut bundles = Vec::with_capacity(memberships.len());
        for membership in memberships {
            let listed =
                self.list_sessions_single_bundle(membership.bundle_name.as_str(), sender_session)?;
            bundles.push(listed);
        }
        Ok(bundles)
    }

    fn home_bundle_name(&self) -> &str {
        self.state.configuration.bundle_paths.bundle_name.as_str()
    }

    fn synthesize_down_bundle(
        &self,
        bundle: &BundleConfiguration,
        relay_socket: &Path,
    ) -> ListedBundle {
        let (state_reason_code, state_reason) = if relay_socket.exists() {
            (
                Some("relay_unavailable".to_string()),
                Some("bundle relay socket is present but relay is unavailable".to_string()),
            )
        } else {
            (
                Some("not_started".to_string()),
                Some("bundle relay socket is not present".to_string()),
            )
        };
        ListedBundle {
            id: bundle.bundle_name.clone(),
            state: ListedBundleState::Down,
            state_reason_code,
            state_reason,
            sessions: list_sessions_from_bundle_configuration(bundle),
        }
    }

    fn request_relay(&self, request: &RelayRequest) -> Result<RelayResponse, std::io::Error> {
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
        let (response, events) = stream_session.request_with_events(request)?;
        if !events.is_empty() {
            emit_inscription(
                "mcp.tool.stream.events_ignored",
                &json!({
                    "bundle_name": self.state.configuration.bundle_paths.bundle_name,
                    "count": events.len(),
                }),
            );
        }
        Ok(response)
    }
}

#[tool_handler]
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("agentmux MCP server for tmux-backed multi-agent coordination.")
    }
}

fn list_sessions_from_bundle_configuration(bundle: &BundleConfiguration) -> Vec<ListedSession> {
    bundle
        .members
        .iter()
        .map(|member| ListedSession {
            id: member.id.clone(),
            name: member.name.clone(),
            transport: match member.target {
                TargetConfiguration::Tmux(_) => ListedSessionTransport::Tmux,
                TargetConfiguration::Acp(_) => ListedSessionTransport::Acp,
            },
        })
        .collect::<Vec<_>>()
}

/// Runs the MCP stdio service and blocks until shutdown.
pub async fn run(configuration: McpConfiguration) -> Result<()> {
    let server = McpServer::new(configuration);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn validate_list_request(params: &ListParams) -> Result<(), McpError> {
    let command = params
        .command
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            validation_tool_error(
                "validation_invalid_params",
                "command is required and must equal \"sessions\"",
                None,
            )
        })?;
    if command != LIST_COMMAND_SESSIONS {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "command is required and must equal \"sessions\"",
            Some(json!({"command": params.command})),
        ));
    }
    if params.args.all && params.args.bundle_name.is_some() {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name and all=true are mutually exclusive",
            None,
        ));
    }
    if let Some(bundle_name) = params.args.bundle_name.as_ref()
        && bundle_name.trim().is_empty()
    {
        return Err(validation_tool_error(
            "validation_invalid_params",
            "bundle_name must be non-empty when provided",
            None,
        ));
    }
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
    if matches!(params.acp_turn_timeout_ms, Some(0)) {
        return Err(validation_tool_error(
            "validation_invalid_acp_turn_timeout",
            "acp_turn_timeout_ms must be greater than zero milliseconds",
            None,
        ));
    }
    if params.quiescence_timeout_ms.is_some() && params.acp_turn_timeout_ms.is_some() {
        return Err(validation_tool_error(
            "validation_conflicting_timeout_fields",
            "quiescence_timeout_ms and acp_turn_timeout_ms are mutually exclusive",
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
        && !(LOOK_LINES_MIN..=LOOK_LINES_MAX).contains(&lines)
    {
        return Err(validation_tool_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": lines,
                "min": LOOK_LINES_MIN,
                "max": LOOK_LINES_MAX,
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

fn map_configuration_error(source: ConfigurationError) -> McpError {
    match source {
        ConfigurationError::UnknownBundle { bundle_name, path } => {
            let message = format!(
                "bundle '{}' is not configured under {}",
                bundle_name,
                path.display()
            );
            validation_tool_error(
                "validation_unknown_bundle",
                message.as_str(),
                Some(json!({
                    "bundle_name": bundle_name,
                    "path": path.display().to_string(),
                })),
            )
        }
        ConfigurationError::InvalidConfiguration { path, message } => validation_tool_error(
            "validation_invalid_arguments",
            "bundle configuration is invalid",
            Some(json!({
                "path": path.display().to_string(),
                "cause": message,
            })),
        ),
        ConfigurationError::InvalidGroupName { path, group_name } => validation_tool_error(
            "validation_invalid_group_name",
            "bundle configuration has invalid group name",
            Some(json!({
                "path": path.display().to_string(),
                "group_name": group_name,
            })),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => validation_tool_error(
            "validation_reserved_group_name",
            "bundle configuration uses reserved group name",
            Some(json!({
                "path": path.display().to_string(),
                "group_name": group_name,
            })),
        ),
        ConfigurationError::AmbiguousSender {
            working_directory,
            matches,
        } => validation_tool_error(
            "validation_ambiguous_sender",
            "sender session selection is ambiguous",
            Some(json!({
                "working_directory": working_directory.display().to_string(),
                "matches": matches,
            })),
        ),
        ConfigurationError::Io { context, source } => internal_tool_error(
            "internal_unexpected_failure",
            "failed to load bundle configuration",
            Some(json!({
                "context": context,
                "cause": source.to_string(),
            })),
        ),
    }
}

fn map_runtime_error(source: RuntimeError) -> McpError {
    match source {
        RuntimeError::InvalidBundleName { bundle_name } => validation_tool_error(
            "validation_invalid_params",
            "bundle_name contains unsupported characters",
            Some(json!({"bundle_name": bundle_name})),
        ),
        other => internal_tool_error(
            "internal_unexpected_failure",
            "failed to resolve bundle runtime paths",
            Some(json!({"cause": other.to_string()})),
        ),
    }
}

fn map_relay_request_failure(socket_path: &Path, source: std::io::Error) -> McpError {
    if is_relay_timeout_error(&source) {
        return internal_tool_error(
            "relay_timeout",
            "relay timed out; relay may be saturated or unresponsive",
            Some(json!({
                "relay_socket": socket_path,
                "io_error_kind": format!("{:?}", source.kind()),
                "cause": source.to_string(),
            })),
        );
    }
    if is_relay_unavailable_error(&source) {
        return internal_tool_error(
            "relay_unavailable",
            "relay is unavailable; start agentmux host relay with matching state-directory",
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

fn is_relay_timeout_error(source: &std::io::Error) -> bool {
    matches!(source.kind(), std::io::ErrorKind::TimedOut)
}

fn is_relay_unavailable_error(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
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
