use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    configuration::{BundleGroupMembership, ConfigurationError, RESERVED_GROUP_ALL},
    relay::{ChatDeliveryMode, RelayError},
    runtime::{
        association::{McpAssociationOverrides, WorkspaceContext},
        error::RuntimeError,
        paths::{RuntimeRootOverrides, RuntimeRoots},
    },
};

use super::{LOOK_LINES_MAXIMUM, LOOK_LINES_MINIMUM, RuntimeArguments};

pub(super) fn parse_runtime_flag(
    arguments: &[String],
    index: &mut usize,
    runtime: &mut RuntimeArguments,
) -> Result<bool, RuntimeError> {
    match arguments[*index].as_str() {
        "--config-directory" => {
            runtime.configuration_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--config-directory",
            )?));
            Ok(true)
        }
        "--state-directory" => {
            runtime.state_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--state-directory",
            )?));
            Ok(true)
        }
        "--inscriptions-directory" | "--logs-directory" => {
            runtime.inscriptions_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--inscriptions-directory",
            )?));
            Ok(true)
        }
        "--repository-root" => {
            runtime.repository_root = Some(PathBuf::from(take_value(
                arguments,
                index,
                "--repository-root",
            )?));
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(super) fn resolve_roots(
    runtime: &RuntimeArguments,
    workspace: &WorkspaceContext,
    local_overrides: Option<&McpAssociationOverrides>,
) -> Result<RuntimeRoots, RuntimeError> {
    let configuration_root = runtime
        .configuration_root
        .clone()
        .or_else(|| local_overrides.and_then(|overrides| overrides.config_root.clone()));
    RuntimeRoots::resolve(&RuntimeRootOverrides {
        configuration_root,
        state_root: runtime.state_root.clone(),
        inscriptions_root: runtime.inscriptions_root.clone(),
        repository_root: runtime
            .repository_root
            .clone()
            .or_else(|| workspace.debug_repository_root()),
    })
}

pub(super) fn parse_delivery_mode(value: &str) -> Result<ChatDeliveryMode, RuntimeError> {
    match value {
        "async" => Ok(ChatDeliveryMode::Async),
        "sync" => Ok(ChatDeliveryMode::Sync),
        _ => Err(RuntimeError::validation(
            "validation_invalid_delivery_mode",
            format!("unsupported delivery mode '{value}'; expected async or sync"),
        )),
    }
}

pub(super) fn parse_positive_u64(
    value: &str,
    flag: &str,
    zero_code: &str,
    zero_message: &str,
) -> Result<u64, RuntimeError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| RuntimeError::InvalidArgument {
            argument: flag.to_string(),
            message: format!("invalid numeric value '{value}'"),
        })?;
    if parsed == 0 {
        return Err(RuntimeError::validation(
            zero_code,
            zero_message.to_string(),
        ));
    }
    Ok(parsed)
}

pub(super) fn parse_look_lines(value: &str) -> Result<u64, RuntimeError> {
    let lines = value.parse::<u64>().map_err(|_| {
        RuntimeError::validation(
            "validation_invalid_lines",
            "lines must be between 1 and 1000".to_string(),
        )
    })?;
    if !(LOOK_LINES_MINIMUM..=LOOK_LINES_MAXIMUM).contains(&lines) {
        return Err(RuntimeError::validation(
            "validation_invalid_lines",
            "lines must be between 1 and 1000".to_string(),
        ));
    }
    Ok(lines)
}

pub(super) fn take_value(
    arguments: &[String],
    index: &mut usize,
    flag: &str,
) -> Result<String, RuntimeError> {
    *index += 1;
    let Some(value) = arguments.get(*index) else {
        return Err(RuntimeError::InvalidArgument {
            argument: flag.to_string(),
            message: "missing value".to_string(),
        });
    };
    Ok(value.to_string())
}

pub(super) fn map_reconcile_error(source: RelayError) -> RuntimeError {
    if source.code.starts_with("validation_") {
        return RuntimeError::validation(source.code, source.message);
    }
    let message = source.message.clone();
    RuntimeError::io(message, std::io::Error::other(format!("{source:?}")))
}

pub(super) fn map_bundle_load_error(source: ConfigurationError) -> RuntimeError {
    match source {
        ConfigurationError::UnknownBundle { bundle_name, .. } => RuntimeError::validation(
            "validation_unknown_bundle",
            format!("bundle '{}' is not configured", bundle_name),
        ),
        ConfigurationError::AmbiguousSender { .. } => RuntimeError::validation(
            "validation_unknown_sender",
            "sender association is ambiguous".to_string(),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!(
                "invalid bundle configuration {}: {}",
                path.display(),
                message
            ),
        ),
        ConfigurationError::InvalidGroupName { path, group_name } => RuntimeError::validation(
            "validation_invalid_group_name",
            format!(
                "invalid group '{}' in bundle configuration {}",
                group_name,
                path.display()
            ),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => RuntimeError::validation(
            "validation_reserved_group_name",
            format!(
                "group '{}' is reserved in bundle configuration {}",
                group_name,
                path.display()
            ),
        ),
        ConfigurationError::Io { context, source } => RuntimeError::io(context, source),
    }
}

pub(super) fn map_relay_error(error: RelayError) -> RuntimeError {
    if error.code.starts_with("validation_") || error.code == "authorization_forbidden" {
        return RuntimeError::validation(error.code, error.message);
    }
    RuntimeError::io(
        error.message,
        std::io::Error::other("relay returned internal error"),
    )
}

pub(super) fn map_relay_request_failure(
    socket_path: &Path,
    source: std::io::Error,
) -> RuntimeError {
    if is_relay_timeout_error(&source) {
        return RuntimeError::validation(
            "relay_timeout",
            format!(
                "relay timed out at {}; relay may be saturated or unresponsive",
                socket_path.display()
            ),
        );
    }
    if is_relay_unavailable_error(&source) {
        return RuntimeError::validation(
            "relay_unavailable",
            format!(
                "relay is unavailable at {}; start agentmux host relay with matching state-directory",
                socket_path.display()
            ),
        );
    }
    RuntimeError::io(
        format!("relay request failed for {}", socket_path.display()),
        source,
    )
}

pub(super) fn remove_relay_socket_file(socket_path: &Path) -> Result<(), RuntimeError> {
    match fs::remove_file(socket_path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RuntimeError::io(
            format!("remove relay socket {}", socket_path.display()),
            source,
        )),
    }
}

pub(super) fn runtime_error_reason(source: &RuntimeError) -> (String, String) {
    match source {
        RuntimeError::Validation { code, message } => (code.clone(), message.clone()),
        RuntimeError::InvalidArgument { message, .. } => {
            ("validation_invalid_arguments".to_string(), message.clone())
        }
        _ => ("runtime_startup_failed".to_string(), source.to_string()),
    }
}

fn is_relay_timeout_error(source: &std::io::Error) -> bool {
    matches!(source.kind(), std::io::ErrorKind::TimedOut)
}

fn is_relay_unavailable_error(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

pub(super) fn validate_group_selector_name(group_name: &str) -> Result<(), RuntimeError> {
    if group_name == RESERVED_GROUP_ALL {
        return Ok(());
    }
    if is_custom_group_name(group_name) {
        return Ok(());
    }
    if is_reserved_group_name(group_name) {
        return Err(RuntimeError::validation(
            "validation_invalid_group_name",
            format!(
                "group '{}' is reserved; only '{}' is currently supported",
                group_name, RESERVED_GROUP_ALL
            ),
        ));
    }
    Err(RuntimeError::validation(
        "validation_invalid_group_name",
        format!(
            "group '{}' must be lowercase (custom) or '{}'",
            group_name, RESERVED_GROUP_ALL
        ),
    ))
}

pub(super) fn resolve_group_bundles(
    memberships: Vec<BundleGroupMembership>,
    group_name: &str,
) -> Result<Vec<String>, RuntimeError> {
    if group_name == RESERVED_GROUP_ALL {
        return Ok(memberships
            .into_iter()
            .map(|membership| membership.bundle_name)
            .collect::<Vec<_>>());
    }
    let selected = memberships
        .into_iter()
        .filter(|membership| membership.groups.iter().any(|group| group == group_name))
        .map(|membership| membership.bundle_name)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unknown_group",
            format!("group '{}' is not configured", group_name),
        ));
    }
    Ok(selected)
}

fn is_reserved_group_name(group_name: &str) -> bool {
    group_name.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
    })
}

fn is_custom_group_name(group_name: &str) -> bool {
    group_name.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    })
}
