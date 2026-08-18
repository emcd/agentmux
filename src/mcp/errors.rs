use std::path::Path;

use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::configuration::ConfigurationError;
use crate::relay::RelayError;
use crate::runtime::error::RuntimeError;

use super::validation::{is_relay_timeout_error, is_relay_unavailable_error};

pub(super) fn map_relay_error(error: RelayError) -> McpError {
    if error.code.starts_with("validation_") || error.code == "authorization_forbidden" {
        return validation_tool_error(&error.code, &error.message, error.details);
    }
    internal_tool_error(&error.code, &error.message, error.details)
}

pub(super) fn map_configuration_error(source: ConfigurationError) -> McpError {
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
        ConfigurationError::UnreadableConfigurationLayer { path, source } => validation_tool_error(
            "validation_unreadable_configuration_layer",
            "configuration layer could not be read",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
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

pub(super) fn map_runtime_error(source: RuntimeError) -> McpError {
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

pub(super) fn map_relay_request_failure(socket_path: &Path, source: std::io::Error) -> McpError {
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

/// Canonical remedy string for an unassociated MCP server: the command that
/// starts the server bound to a bundle and session so its relay stream exists.
pub(super) const UNASSOCIATED_SERVER_REMEDY: &str =
    "agentmux host mcp --bundle <name> --session-name <id>";

/// Builds the validation-shaped error every relay-backed tool returns when the
/// server has no resolvable bundle+session association. The relay stream each
/// tool needs is constructed only when both are present, so a single actionable
/// contract — server association — is the caller's remedy, shared by the early
/// prechecks and the `map_relay_stream_failure` chokepoint.
pub(super) fn unassociated_server_error() -> McpError {
    validation_tool_error(
        "validation_unassociated_server",
        "MCP server is not associated with a bundle and session; start it with \
         `agentmux host mcp --bundle <name> --session-name <id>`, or run it from \
         an associated bundle worktree so discovery can resolve them.",
        Some(json!({
            "reason": "unassociated_server",
            "remedy": UNASSOCIATED_SERVER_REMEDY,
        })),
    )
}

/// Builds the error reporting a retained startup fault at tool-invocation time.
///
/// Carries the original code so a caller can distinguish an absent configuration
/// layer from a malformed file from an unknown bundle, rather than seeing one
/// undifferentiated "not ready".
pub(super) fn startup_fault_error(fault: &crate::mcp::McpStartupFault) -> McpError {
    validation_tool_error(
        fault.code.as_str(),
        format!(
            "MCP server started but could not build an operational context: {}",
            fault.message
        )
        .as_str(),
        Some(json!({
            "reason": "startup_fault",
            "startup_fault_code": fault.code,
            "startup_fault_message": fault.message,
        })),
    )
}

pub(super) fn validation_tool_error(
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> McpError {
    McpError::invalid_params(
        message.to_string(),
        Some(error_payload(code, message, details)),
    )
}

pub(super) fn internal_tool_error(
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> McpError {
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
