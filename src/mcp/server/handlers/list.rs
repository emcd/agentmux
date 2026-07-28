//! `list` tool: enumerate principals in one namespace, fan out across
//! namespaces, or read the relay-wide `GLOBAL` registry.

use std::path::Path;

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::json;

use crate::configuration::{
    BundleConfiguration, load_bundle_configuration, load_bundle_group_memberships,
};
use crate::mcp::errors::{
    internal_tool_error, map_configuration_error, map_relay_error, map_relay_request_failure,
    map_runtime_error, validation_tool_error,
};
use crate::mcp::params::{
    LIST_COMMAND_DECISIONS, LIST_COMMAND_NAMESPACES, LIST_COMMAND_PRINCIPALS, LIST_COMMAND_RELAYS,
    LIST_SESSIONS_SCHEMA_VERSION, ListArgs, ListDecisionsArgs, ListNamespacesArgs, ListParams,
    ListRelaysArgs,
};
use crate::mcp::server::{McpServer, service::RelayCallError};
use crate::mcp::validation::{
    is_relay_unavailable_error, parse_meta_tool_args, validate_list_decisions_args,
    validate_list_namespaces_args, validate_list_params, validate_list_principals_args,
    validate_list_relays_args,
};
use crate::relay::{
    ListedBundle, ListedBundleState, ListedSession, RelayRequest, RelayResponse,
    load_startup_failures,
};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{BundleRuntimePaths, RelayRuntimePaths};

#[tool_router(router = tool_router_list, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "list",
        description = "List principals for one namespace or fan out across namespaces."
    )]
    async fn tool_list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_list_params(&params)?;
        let command = params.command.trim();
        match command {
            LIST_COMMAND_PRINCIPALS => self.list_principals(&params),
            LIST_COMMAND_NAMESPACES => self.list_namespaces(&params),
            LIST_COMMAND_RELAYS => self.list_relays(&params),
            LIST_COMMAND_DECISIONS => self.list_decisions(&params),
            other => Err(validation_tool_error(
                "validation_invalid_params",
                "command must be \"principals\", \"namespaces\", \"relays\", or \"decisions\"",
                Some(json!({"command": other})),
            )),
        }
    }

    fn list_principals(&self, params: &ListParams) -> Result<CallToolResult, McpError> {
        let parsed_args = parse_meta_tool_args::<ListArgs>(params.args.clone()).map_err(|reason| {
            validation_tool_error(
                "validation_invalid_params",
                "invalid args for list command",
                Some(json!({
                    "reason": reason,
                    "hint": "pass args as a JSON object; use help query 'list.principals' for exact schema",
                })),
            )
        })?;
        validate_list_principals_args(&parsed_args)?;
        // A relay selector forwards principal discovery to a configured peer;
        // validation has guaranteed a concrete namespace and a non-empty alias.
        if let Some(relay) = parsed_args
            .relay
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return self.list_foreign_principals(relay, &parsed_args);
        }
        let requester_session = self.require_associated_sender_session()?;
        let namespace = parsed_args
            .namespace
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        emit_inscription(
            "mcp.tool.list.request",
            &json!({
                "requester_session": requester_session,
                "command": LIST_COMMAND_PRINCIPALS,
                "namespace": namespace,
            }),
        );
        let mut bundles = match namespace.as_deref() {
            Some("*") => self.list_sessions_all_bundles(requester_session.as_str())?,
            Some("GLOBAL") => Vec::new(),
            Some(bundle_name) => {
                vec![self.list_sessions_single_bundle(bundle_name, requester_session.as_str())?]
            }
            None => {
                let bundle_name = self
                    .home_bundle_name()
                    .map(ToString::to_string)
                    .ok_or_else(|| {
                        validation_tool_error(
                            "validation_unknown_bundle",
                            "namespace is required when MCP server is not associated with a bundle",
                            None,
                        )
                    })?;
                vec![self.list_sessions_single_bundle(
                    bundle_name.as_str(),
                    requester_session.as_str(),
                )?]
            }
        };
        bundles.push(self.list_global_sessions(requester_session.as_str())?);
        let response = json!({
            "schema_version": LIST_SESSIONS_SCHEMA_VERSION,
            "bundles": bundles,
        });
        emit_inscription(
            "mcp.tool.list.success",
            &json!({
                "namespace": namespace,
                "bundle_count": response["bundles"].as_array().map_or(0, |value| value.len()),
            }),
        );
        Ok(CallToolResult::success(vec![Content::json(response)?]))
    }

    fn list_decisions(&self, params: &ListParams) -> Result<CallToolResult, McpError> {
        let args =
            parse_meta_tool_args::<ListDecisionsArgs>(params.args.clone()).map_err(|reason| {
                validation_tool_error(
                    "validation_invalid_params",
                    "invalid args for list decisions command",
                    Some(json!({
                        "reason": reason,
                        "hint": "pass args as a JSON object; use help query 'list.decisions' for exact schema",
                    })),
                )
            })?;
        validate_list_decisions_args(&args)?;
        emit_inscription(
            "mcp.tool.list.decisions.request",
            &json!({
                "namespace": self.associated_namespace(),
            }),
        );
        let request = RelayRequest::ChoicesList;
        match self.request_relay(&request) {
            Ok(RelayResponse::ChoicesList {
                schema_version,
                pending_requests,
            }) => {
                let pending_count = pending_requests.len();
                let response = json!({
                    "schema_version": schema_version,
                    "pending_requests": pending_requests,
                });
                emit_inscription(
                    "mcp.tool.list.decisions.success",
                    &json!({
                        "pending_count": pending_count,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(RelayResponse::Error { error }) => {
                emit_inscription(
                    "mcp.tool.list.decisions.relay_error",
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
                    "mcp.tool.list.decisions.unexpected_response",
                    &json!({"response": other}),
                );
                Err(internal_tool_error(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({"response": other})),
                ))
            }
            Err(source) => {
                Err(self.map_relay_call_error("mcp.tool.list.decisions.io_error", source))
            }
        }
    }

    /// Forwards principal discovery to a configured foreign relay. The relay
    /// engine authorizes the requester's `list` control and applies the peer's
    /// ingress scope; this layer echoes the origin-local `relay` selector back to
    /// the caller and passes peer-authored bundles through unchanged.
    fn list_foreign_principals(
        &self,
        relay: &str,
        parsed_args: &ListArgs,
    ) -> Result<CallToolResult, McpError> {
        self.require_association()?;
        let namespace = parsed_args
            .namespace
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .expect("foreign principal discovery validated a concrete namespace");
        emit_inscription(
            "mcp.tool.list.principals.request",
            &json!({"relay": relay, "namespace": namespace}),
        );
        let request = RelayRequest::DiscoverPrincipals {
            relay: Some(relay.to_string()),
            namespace: namespace.to_string(),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::DiscoverPrincipals {
                schema_version,
                bundles,
            }) => {
                let bundle_count = bundles.len();
                let response = json!({
                    "schema_version": schema_version,
                    "relay": relay,
                    "bundles": bundles,
                });
                emit_inscription(
                    "mcp.tool.list.principals.success",
                    &json!({"relay": relay, "namespace": namespace, "bundle_count": bundle_count}),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.list.principals", other)),
            Err(source) => {
                Err(self.map_relay_call_error("mcp.tool.list.principals.io_error", source))
            }
        }
    }

    fn list_namespaces(&self, params: &ListParams) -> Result<CallToolResult, McpError> {
        let args =
            parse_meta_tool_args::<ListNamespacesArgs>(params.args.clone()).map_err(|reason| {
                validation_tool_error(
                    "validation_invalid_params",
                    "invalid args for list namespaces command",
                    Some(json!({
                        "reason": reason,
                        "hint": "pass args as a JSON object; use help query 'list.namespaces' for exact schema",
                    })),
                )
            })?;
        validate_list_namespaces_args(&args)?;
        self.require_association()?;
        let relay = args
            .relay
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        emit_inscription("mcp.tool.list.namespaces.request", &json!({"relay": relay}));
        let request = RelayRequest::DiscoverNamespaces {
            relay: relay.map(str::to_string),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::DiscoverNamespaces {
                schema_version,
                namespaces,
            }) => {
                let namespace_count = namespaces.len();
                // Foreign responses echo the selected relay; local responses omit
                // it (per the discovery contract).
                let mut response = json!({
                    "schema_version": schema_version,
                    "namespaces": namespaces,
                });
                if let Some(relay) = relay {
                    response["relay"] = json!(relay);
                }
                emit_inscription(
                    "mcp.tool.list.namespaces.success",
                    &json!({"relay": relay, "namespace_count": namespace_count}),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.list.namespaces", other)),
            Err(source) => {
                Err(self.map_relay_call_error("mcp.tool.list.namespaces.io_error", source))
            }
        }
    }

    fn list_relays(&self, params: &ListParams) -> Result<CallToolResult, McpError> {
        let args = parse_meta_tool_args::<ListRelaysArgs>(params.args.clone()).map_err(|reason| {
            validation_tool_error(
                "validation_invalid_params",
                "invalid args for list relays command",
                Some(json!({
                    "reason": reason,
                    "hint": "pass args as a JSON object; use help query 'list.relays' for exact schema",
                })),
            )
        })?;
        validate_list_relays_args(&args)?;
        self.require_association()?;
        emit_inscription("mcp.tool.list.relays.request", &json!({}));
        let request = RelayRequest::ListRelays;
        match self.request_relay(&request) {
            Ok(RelayResponse::ListRelays {
                schema_version,
                relays,
            }) => {
                let relay_count = relays.len();
                let response = json!({
                    "schema_version": schema_version,
                    "relays": relays,
                });
                emit_inscription(
                    "mcp.tool.list.relays.success",
                    &json!({"relay_count": relay_count}),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.list.relays", other)),
            Err(source) => Err(self.map_relay_call_error("mcp.tool.list.relays.io_error", source)),
        }
    }

    fn list_sessions_single_bundle(
        &self,
        bundle_name: &str,
        requester_session: &str,
    ) -> Result<ListedBundle, McpError> {
        let bundle_paths =
            BundleRuntimePaths::resolve(&self.state.configuration.state_root, bundle_name)
                .map_err(map_runtime_error)?;
        let relay_paths = RelayRuntimePaths::resolve(&self.state.configuration.state_root);
        let bundle =
            load_bundle_configuration(&self.state.configuration.configuration_root, bundle_name)
                .map_err(map_configuration_error)?;
        let relay_socket = relay_paths.relay_socket;
        let request = RelayRequest::List {
            requester_session: Some(requester_session.to_string()),
        };
        // Route through the persistent stream session so this call shares the
        // MCP server's cached Hello-registered connection. Issuing a fresh
        // single-shot Hello with the same `principal_id` would conflict with
        // that registration (`runtime_identity_claim_conflict`) and time out.
        match self.request_relay_with_namespace(&request, Some(bundle_name)) {
            Ok(RelayResponse::List { bundle, .. }) => Ok(bundle),
            Ok(RelayResponse::Error { error }) => Err(map_relay_error(error)),
            Ok(other) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({"response": other})),
            )),
            // Only a present-but-unreachable relay earns the synthesized down
            // view. A retained startup fault never reached the relay at all, so
            // papering it over with an empty bundle would hide the defect behind
            // a plausible answer.
            Err(RelayCallError::Io(source))
                if is_relay_unavailable_error(&source)
                    && self.home_bundle_name() == Some(bundle_name) =>
            {
                Ok(self.synthesize_down_bundle(&bundle, &relay_socket, &bundle_paths))
            }
            Err(RelayCallError::Io(source)) => {
                Err(map_relay_request_failure(&relay_socket, source))
            }
            Err(RelayCallError::NotReady(fault)) => Err(fault),
        }
    }

    fn list_sessions_all_bundles(
        &self,
        requester_session: &str,
    ) -> Result<Vec<ListedBundle>, McpError> {
        let memberships =
            load_bundle_group_memberships(&self.state.configuration.configuration_root)
                .map_err(map_configuration_error)?;
        let mut bundles = Vec::with_capacity(memberships.len());
        for membership in memberships {
            let listed = self
                .list_sessions_single_bundle(membership.bundle_name.as_str(), requester_session)?;
            bundles.push(listed);
        }
        Ok(bundles)
    }

    /// Issues a `List` with the wire-envelope namespace pinned to `GLOBAL`.
    ///
    /// The relay intercepts this path and returns a synthetic `ListedBundle`
    /// (`id = "GLOBAL"`) whose sessions are relay-wide principals (operators,
    /// external apps, peer relays). An empty registry yields a `Down` bundle
    /// with no sessions, not an error. A relay-unavailable failure mirrors
    /// the home-bundle fallback: the response carries an empty `GLOBAL` view
    /// rather than failing the entire `list` call.
    fn list_global_sessions(&self, requester_session: &str) -> Result<ListedBundle, McpError> {
        let request = RelayRequest::List {
            requester_session: Some(requester_session.to_string()),
        };
        match self.request_relay_with_namespace(&request, Some("GLOBAL")) {
            Ok(RelayResponse::List { bundle, .. }) => Ok(bundle),
            Ok(RelayResponse::Error { error }) => Err(map_relay_error(error)),
            Ok(other) => Err(internal_tool_error(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({"response": other})),
            )),
            Err(RelayCallError::Io(source)) if is_relay_unavailable_error(&source) => {
                Ok(synthesize_empty_global_bundle())
            }
            Err(source) => Err(self.map_relay_call_error("mcp.tool.list.global.io_error", source)),
        }
    }

    fn home_bundle_name(&self) -> Option<&str> {
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
            principals: list_sessions_from_bundle_configuration(bundle),
            principals_partial: None,
        }
    }
}

fn synthesize_empty_global_bundle() -> ListedBundle {
    ListedBundle {
        id: "GLOBAL".to_string(),
        hosted: false,
        state: ListedBundleState::Down,
        startup_health: None,
        state_reason_code: Some("relay_unavailable".to_string()),
        state_reason: Some(
            "relay is unavailable; GLOBAL session registry not enumerable".to_string(),
        ),
        startup_failure_count: 0,
        recent_startup_failures: Vec::new(),
        principals: Vec::new(),
        principals_partial: None,
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
            ready: false,
        })
        .collect::<Vec<_>>()
}
