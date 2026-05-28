//! MCP server surface for agentmux.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::{Value, json};

use crate::configuration::{
    BundleConfiguration, load_bundle_configuration, load_bundle_group_memberships,
};
use crate::relay::{
    ListedBundle, ListedBundleState, ListedSession, LookSnapshotPayload, RelayRequest,
    RelayResponse, RelayStreamSession, load_startup_failures, request_relay,
};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{BundleRuntimePaths, RelayRuntimePaths};

use super::errors::{
    internal_tool_error, map_configuration_error, map_relay_error, map_relay_request_failure,
    map_runtime_error, validation_tool_error,
};
use super::help::help_tool;
use super::params::{
    GRANT_COMMAND_LIST, GRANT_COMMAND_RESOLVE, GrantListArgs, GrantParams, GrantResolveArgs,
    HelpParams, LIFECYCLE_COMMAND_DOWN, LIFECYCLE_COMMAND_UP, LIST_COMMAND_SESSIONS,
    LIST_SESSIONS_SCHEMA_VERSION, LifecycleArgs, LifecycleParams, ListArgs, ListParams, LookParams,
    RawwParams, SendParams,
};
use super::validation::{
    is_relay_unavailable_error, parse_meta_tool_args, validate_grant_list_args,
    validate_grant_params, validate_grant_resolve_args, validate_help_request,
    validate_lifecycle_args, validate_lifecycle_params, validate_list_request,
    validate_look_request, validate_raww_request, validate_send_request,
};

/// Configuration provided when booting MCP stdio service.
#[derive(Clone, Debug)]
pub struct McpConfiguration {
    pub configuration_root: PathBuf,
    pub state_root: PathBuf,
    pub associated_bundle_paths: Option<BundleRuntimePaths>,
    pub sender_session: Option<String>,
}

#[derive(Clone, Debug)]
struct McpServer {
    state: Arc<McpState>,
}

#[derive(Debug)]
struct McpState {
    configuration: McpConfiguration,
    relay_stream: Mutex<Option<RelayStreamSession>>,
}

#[tool_router]
impl McpServer {
    fn new(configuration: McpConfiguration) -> Self {
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
        Self {
            state: Arc::new(McpState {
                configuration,
                relay_stream: Mutex::new(relay_stream),
            }),
        }
    }

    #[tool(description = "List sessions for one bundle or fan out across bundles.")]
    async fn list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        let parsed_args = parse_meta_tool_args::<ListArgs>(params.args.clone()).map_err(|reason| {
            validation_tool_error(
                "validation_invalid_params",
                "invalid args for list command",
                Some(json!({
                    "reason": reason,
                    "hint": "pass args as a JSON object; use help query 'list.sessions' for exact schema",
                })),
            )
        })?;
        validate_list_request(&params, &parsed_args)?;
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
        let selected_bundle = parsed_args
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
                "all": parsed_args.all,
            }),
        );
        if parsed_args.all {
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
            .or_else(|| self.home_bundle_name().map(ToString::to_string))
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_unknown_bundle",
                    "bundle_name is required when MCP server is not associated with a bundle",
                    None,
                )
            })?;
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

    #[tool(
        description = "Return tool/command help and JSON schemas. Query omitted or `agentmux` for tool list, `list` for list meta-tool commands, or `list.sessions`/`send`/`look`/`raww` for exact schemas."
    )]
    async fn help(
        &self,
        Parameters(params): Parameters<HelpParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_help_request(&params)?;
        Ok(CallToolResult::success(vec![Content::json(help_tool(
            params,
        )?)?]))
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
                "bundle_name": self.associated_bundle_name(),
                "request_id": params.request_id.clone(),
                "targets": params.targets.clone(),
                "broadcast": params.broadcast,
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

        let request = RelayRequest::Send {
            request_id: params.request_id.clone(),
            sender_session,
            message: params.message.clone(),
            targets: params.targets.clone(),
            broadcast: params.broadcast,
            quiet_window_ms: None,
            quiescence_timeout_ms: params.quiescence_timeout_ms,
            acp_turn_timeout_ms: params.acp_turn_timeout_ms,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Send {
                schema_version,
                bundle_name,
                request_id,
                sender_session,
                sender_display_name,
                results,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "request_id": request_id,
                    "sender_session": sender_session,
                    "sender_display_name": sender_display_name,
                    "results": results,
                });
                emit_inscription(
                    "mcp.tool.send.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
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
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.send.io_error", source)),
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
                "bundle_name": self.associated_bundle_name(),
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
                snapshot,
            }) => {
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                response_map.insert("bundle_name".to_string(), Value::String(bundle_name));
                response_map.insert(
                    "requester_session".to_string(),
                    Value::String(requester_session),
                );
                response_map.insert("target_session".to_string(), Value::String(target_session));
                response_map.insert("captured_at".to_string(), Value::String(captured_at));
                let snapshot_count = match snapshot {
                    LookSnapshotPayload::Lines { snapshot_lines } => {
                        let count = snapshot_lines.len();
                        response_map.insert("snapshot_format".to_string(), json!("lines"));
                        response_map.insert("snapshot_lines".to_string(), json!(snapshot_lines));
                        count
                    }
                    LookSnapshotPayload::AcpEntriesV1 {
                        snapshot_entries,
                        freshness,
                        snapshot_source,
                        stale_reason_code,
                        snapshot_age_ms,
                    } => {
                        let count = snapshot_entries.len();
                        response_map.insert("snapshot_format".to_string(), json!("acp_entries_v1"));
                        response_map
                            .insert("snapshot_entries".to_string(), json!(snapshot_entries));
                        response_map.insert("freshness".to_string(), json!(freshness));
                        response_map.insert("snapshot_source".to_string(), json!(snapshot_source));
                        if let Some(value) = stale_reason_code {
                            response_map
                                .insert("stale_reason_code".to_string(), Value::String(value));
                        }
                        if let Some(value) = snapshot_age_ms {
                            response_map.insert("snapshot_age_ms".to_string(), json!(value));
                        }
                        count
                    }
                };
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.look.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
                        "requester_session": response["requester_session"],
                        "target_session": response["target_session"],
                        "snapshot_format": response["snapshot_format"],
                        "snapshot_count": snapshot_count,
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
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.look.io_error", source)),
        }
    }

    #[tool(description = "Write raw text directly to one target session.")]
    async fn raww(
        &self,
        Parameters(params): Parameters<RawwParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_raww_request(&params)?;
        emit_inscription(
            "mcp.tool.raww.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "request_id": params.request_id.clone(),
                "target_session": params.target_session.clone(),
                "text_length": params.text.len(),
                "no_enter": params.no_enter,
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

        let request = RelayRequest::Raww {
            request_id: params.request_id.clone(),
            sender_session,
            target_session: params.target_session.clone(),
            text: params.text.clone(),
            no_enter: params.no_enter,
            bundle_name: None,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Raww {
                schema_version,
                status,
                target_session,
                transport,
                request_id,
                message_id,
                details,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "status": status,
                    "target_session": target_session,
                    "transport": transport,
                    "request_id": request_id,
                    "message_id": message_id,
                    "details": details,
                });
                emit_inscription(
                    "mcp.tool.raww.success",
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "status": response["status"],
                        "target_session": response["target_session"],
                        "transport": response["transport"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.raww.relay_error",
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
                    "mcp.tool.raww.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.raww.io_error", source)),
        }
    }

    #[tool(
        description = "Inspect or resolve pending ACP permission requests. Use command=\"list\" to enumerate pending requests, command=\"resolve\" to decide one."
    )]
    async fn grant(
        &self,
        Parameters(params): Parameters<GrantParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_grant_params(&params)?;
        let command = params
            .command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "command is required; allowed values are \"list\" or \"resolve\"",
                    None,
                )
            })?;
        match command {
            GRANT_COMMAND_LIST => {
                let args = parse_meta_tool_args::<GrantListArgs>(params.args.clone())
                    .map_err(|reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for grant list command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'grant.list' for exact schema",
                            })),
                        )
                    })?;
                self.grant_list(args)
            }
            GRANT_COMMAND_RESOLVE => {
                let args = parse_meta_tool_args::<GrantResolveArgs>(params.args.clone())
                    .map_err(|reason| {
                        validation_tool_error(
                            "validation_invalid_params",
                            "invalid args for grant resolve command",
                            Some(json!({
                                "reason": reason,
                                "hint": "pass args as a JSON object; use help query 'grant.resolve' for exact schema",
                            })),
                        )
                    })?;
                self.grant_resolve(args)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "grant command must be \"list\" or \"resolve\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn grant_list(&self, args: GrantListArgs) -> Result<CallToolResult, McpError> {
        validate_grant_list_args(&args)?;
        emit_inscription(
            "mcp.tool.grant.list.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
            }),
        );
        let request = RelayRequest::PermissionList {
            bundle_name: args.bundle_name.clone(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::PermissionList {
                schema_version,
                bundle_name,
                pending_requests,
            }) => {
                let pending_count = pending_requests.len();
                let response = json!({
                    "schema_version": schema_version,
                    "bundle_name": bundle_name,
                    "pending_requests": pending_requests,
                });
                emit_inscription(
                    "mcp.tool.grant.list.success",
                    &json!({
                        "bundle_name": response["bundle_name"],
                        "pending_count": pending_count,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.grant.list.relay_error",
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
                    "mcp.tool.grant.list.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.grant.list.io_error", source))
            }
        }
    }

    fn grant_resolve(&self, args: GrantResolveArgs) -> Result<CallToolResult, McpError> {
        validate_grant_resolve_args(&args)?;
        let permission_request_id = args
            .permission_request_id
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let outcome = args
            .outcome
            .as_ref()
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let option_id = args
            .option_id
            .as_ref()
            .map(|value| value.trim().to_string());
        emit_inscription(
            "mcp.tool.grant.resolve.request",
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "permission_request_id": permission_request_id,
                "outcome": outcome,
                "has_option_id": option_id.is_some(),
            }),
        );
        let request = RelayRequest::PermissionResolve {
            permission_request_id: permission_request_id.clone(),
            outcome: outcome.clone(),
            option_id: option_id.clone(),
            bundle_name: args.bundle_name.clone(),
            ui_session_id: None,
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::PermissionDecision {
                schema_version,
                status,
                permission_request_id,
                outcome,
                reason_code,
                reason,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "status": status,
                    "permission_request_id": permission_request_id,
                    "outcome": outcome,
                    "reason_code": reason_code,
                    "reason": reason,
                });
                emit_inscription(
                    "mcp.tool.grant.resolve.success",
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "permission_request_id": response["permission_request_id"],
                        "status": response["status"],
                        "outcome": response["outcome"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.grant.resolve.relay_error",
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
                    "mcp.tool.grant.resolve.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_stream_failure("mcp.tool.grant.resolve.io_error", source))
            }
        }
    }

    #[tool(
        description = "Administer bundle runtime lifecycle. Use command=\"up\" to host the associated bundle or command=\"down\" to unhost it."
    )]
    async fn lifecycle(
        &self,
        Parameters(params): Parameters<LifecycleParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_lifecycle_params(&params)?;
        let command = params
            .command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                validation_tool_error(
                    "validation_invalid_params",
                    "command is required; allowed values are \"up\" or \"down\"",
                    None,
                )
            })?;
        let args = parse_meta_tool_args::<LifecycleArgs>(params.args.clone()).map_err(|reason| {
            validation_tool_error(
                "validation_invalid_params",
                "invalid args for lifecycle command",
                Some(json!({
                    "reason": reason,
                    "hint": "pass args as a JSON object; use help query 'lifecycle.up' or 'lifecycle.down' for exact schema",
                })),
            )
        })?;
        match command {
            LIFECYCLE_COMMAND_UP => {
                validate_lifecycle_args(&args, LIFECYCLE_COMMAND_UP)?;
                self.lifecycle_transition(LIFECYCLE_COMMAND_UP, RelayRequest::Up)
            }
            LIFECYCLE_COMMAND_DOWN => {
                validate_lifecycle_args(&args, LIFECYCLE_COMMAND_DOWN)?;
                self.lifecycle_transition(LIFECYCLE_COMMAND_DOWN, RelayRequest::Down)
            }
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "lifecycle command must be \"up\" or \"down\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn lifecycle_transition(
        &self,
        command: &str,
        request: RelayRequest,
    ) -> Result<CallToolResult, McpError> {
        let request_event = format!("mcp.tool.lifecycle.{command}.request");
        let success_event = format!("mcp.tool.lifecycle.{command}.success");
        let relay_error_event = format!("mcp.tool.lifecycle.{command}.relay_error");
        let unexpected_event = format!("mcp.tool.lifecycle.{command}.unexpected_response");
        let io_error_event = format!("mcp.tool.lifecycle.{command}.io_error");
        emit_inscription(
            request_event.as_str(),
            &json!({
                "bundle_name": self.associated_bundle_name(),
                "command": command,
            }),
        );
        match self.request_relay(&request) {
            Ok(RelayResponse::BundleTransition {
                schema_version,
                action,
                bundles,
                changed_bundle_count,
                skipped_bundle_count,
                failed_bundle_count,
                changed_any,
            }) => {
                let response = json!({
                    "schema_version": schema_version,
                    "action": action,
                    "bundles": bundles,
                    "changed_bundle_count": changed_bundle_count,
                    "skipped_bundle_count": skipped_bundle_count,
                    "failed_bundle_count": failed_bundle_count,
                    "changed_any": changed_any,
                });
                emit_inscription(
                    success_event.as_str(),
                    &json!({
                        "bundle_name": self.associated_bundle_name(),
                        "action": response["action"],
                        "changed_bundle_count": response["changed_bundle_count"],
                        "skipped_bundle_count": response["skipped_bundle_count"],
                        "failed_bundle_count": response["failed_bundle_count"],
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    relay_error_event.as_str(),
                    &json!({
                        "code": error.code.clone(),
                        "message": error.message.clone(),
                        "details": error.details.clone(),
                    }),
                );
                Err(map_relay_error(error))
            }
            Ok(other) => {
                emit_inscription(unexpected_event.as_str(), &json!({"response": other}));
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => Err(self.map_relay_stream_failure(io_error_event.as_str(), source)),
        }
    }

    fn list_sessions_single_bundle(
        &self,
        bundle_name: &str,
        sender_session: &str,
    ) -> Result<ListedBundle, McpError> {
        let bundle_paths =
            BundleRuntimePaths::resolve(&self.state.configuration.state_root, bundle_name)
                .map_err(map_runtime_error)?;
        let relay_paths = RelayRuntimePaths::resolve(&self.state.configuration.state_root);
        let bundle =
            load_bundle_configuration(&self.state.configuration.configuration_root, bundle_name)
                .map_err(map_configuration_error)?;
        let relay_socket = relay_paths.relay_socket;
        match request_relay(
            &relay_socket,
            bundle_name,
            sender_session,
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
                    && self.home_bundle_name() == Some(bundle_name) =>
            {
                Ok(self.synthesize_down_bundle(&bundle, &relay_socket, &bundle_paths))
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

    fn home_bundle_name(&self) -> Option<&str> {
        self.state
            .configuration
            .associated_bundle_paths
            .as_ref()
            .map(|paths| paths.bundle_name.as_str())
    }

    fn associated_bundle_name(&self) -> Option<&str> {
        self.state
            .configuration
            .associated_bundle_paths
            .as_ref()
            .map(|paths| paths.bundle_name.as_str())
    }

    fn synthesize_down_bundle(
        &self,
        bundle: &BundleConfiguration,
        relay_socket: &Path,
        bundle_paths: &BundleRuntimePaths,
    ) -> ListedBundle {
        let (state_reason_code, state_reason) = if relay_socket.exists() {
            (
                Some("relay_unavailable".to_string()),
                Some("relay socket is present but relay is unavailable".to_string()),
            )
        } else {
            (
                Some("not_started".to_string()),
                Some("relay socket is not present".to_string()),
            )
        };
        let (startup_failure_count, recent_startup_failures) =
            match load_startup_failures(&bundle_paths.runtime_directory) {
                Ok(records) => (records.len(), records),
                Err(_) => (0, Vec::new()),
            };
        ListedBundle {
            id: bundle.bundle_name.clone(),
            hosted: false,
            state: ListedBundleState::Down,
            startup_health: None,
            state_reason_code,
            state_reason,
            startup_failure_count,
            recent_startup_failures,
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
                    "bundle_name": self.associated_bundle_name(),
                    "count": events.len(),
                }),
            );
        }
        Ok(response)
    }

    fn map_relay_stream_failure(&self, event: &str, source: std::io::Error) -> McpError {
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
            transport: member.target.session_type().into(),
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
