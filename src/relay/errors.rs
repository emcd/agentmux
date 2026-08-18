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
        ConfigurationError::UnreadableConfigurationLayer { path, source } => relay_error(
            UNREADABLE_LAYER_CODE,
            "configuration layer could not be read",
            Some(json!({"path": path, "cause": source.to_string()})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration could not be loaded",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
    }
}

/// The code an unreadable layer keeps wherever it crosses a boundary.
///
/// A caller cannot match on the `ConfigurationError` variant once the error has
/// been mapped, so the code is what carries the distinction the whole change
/// exists to preserve. Folding it into a general validation code would restore
/// the ambiguity one layer up from where it was removed.
pub(super) const UNREADABLE_LAYER_CODE: &str = "validation_unreadable_configuration_layer";

pub(super) fn relay_error(code: &str, message: &str, details: Option<Value>) -> RelayError {
    RelayError {
        code: code.to_string(),
        message: message.to_string(),
        details,
    }
}

/// Rejection code for a resolved target whose transport type does not support
/// the requested operation (Transport Capability Contract). Distinct from
/// `validation_unsupported_namespace` (reserved namespace names no session at
/// all) and `validation_unknown_target` (target not configured): the target
/// exists and is routable, but its transport cannot perform this operation.
pub(super) const VALIDATION_UNSUPPORTED_OPERATION: &str = "validation_unsupported_operation";

/// Builds the capability-gate rejection for a look/raww target whose transport
/// lacks the operation's capability. `capability_flag` names the failed flag
/// (`can_be_looked` / `can_be_written`) and is reported as a `false`-valued
/// detail key so the diagnostic is actionable.
pub(super) fn unsupported_operation(
    target_session: &str,
    session_type: SessionType,
    capability_flag: &str,
) -> RelayError {
    let mut details = serde_json::Map::new();
    details.insert("target_session".to_string(), json!(target_session));
    details.insert("session_type".to_string(), json!(session_type));
    details.insert(capability_flag.to_string(), json!(false));
    relay_error(
        VALIDATION_UNSUPPORTED_OPERATION,
        "target transport does not support this operation",
        Some(Value::Object(details)),
    )
}

/// Builds the structured error for a session whose declared session type does
/// not yet have an implemented delivery path. With UI promoted to a first-class
/// transport, the only such type is the forward-declared `Pubsub` stub: a
/// configured Pubsub target is constructed as `TransportImpl::Pubsub` and yields
/// this terminal outcome rather than being misrouted to another transport.
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
        ConfigurationError::UnreadableConfigurationLayer { path, source } => relay_error(
            UNREADABLE_LAYER_CODE,
            "configuration layer could not be read",
            Some(json!({"path": path, "cause": source.to_string()})),
        ),
        other => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({"cause": other.to_string()})),
        ),
    }
}
