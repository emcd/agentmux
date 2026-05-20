use serde_json::{Value, json};

use crate::configuration::{ConfigurationError, SessionType};

use super::RelayError;

pub(super) fn map_config(error: ConfigurationError) -> RelayError {
    match error {
        ConfigurationError::UnknownBundle { bundle_name, path } => relay_error(
            "validation_unknown_bundle",
            "bundle is not configured",
            Some(json!({"bundle_name": bundle_name, "path": path})),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration is invalid",
            Some(json!({"path": path, "cause": message})),
        ),
        ConfigurationError::InvalidGroupName { path, group_name } => relay_error(
            "validation_invalid_group_name",
            "bundle configuration uses invalid group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => relay_error(
            "validation_reserved_group_name",
            "bundle configuration uses reserved group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::AmbiguousSender {
            working_directory,
            matches,
        } => relay_error(
            "validation_unknown_sender",
            "sender association is ambiguous",
            Some(json!({"working_directory": working_directory, "matches": matches})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration could not be loaded",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
    }
}

pub(super) fn relay_error(code: &str, message: &str, details: Option<Value>) -> RelayError {
    RelayError {
        code: code.to_string(),
        message: message.to_string(),
        details,
    }
}

/// Builds the structured error for a session whose declared session type does
/// not yet have an implemented delivery path (`ui`, `pubsub`).
pub(super) fn session_type_not_implemented(
    session_id: &str,
    session_type: SessionType,
) -> RelayError {
    relay_error(
        "runtime_session_type_not_implemented",
        "session type delivery is not yet implemented",
        Some(json!({
            "session_id": session_id,
            "session_type": session_type,
        })),
    )
}

pub(super) fn map_tui_config(error: ConfigurationError) -> RelayError {
    match error {
        ConfigurationError::InvalidConfiguration { path, message } => relay_error(
            "validation_invalid_arguments",
            "tui configuration is invalid",
            Some(json!({"path": path, "cause": message})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
        other => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({"cause": other.to_string()})),
        ),
    }
}
