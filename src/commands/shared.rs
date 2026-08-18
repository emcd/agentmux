use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    configuration::{
        BundleGroupMembership, ConfigurationError, ConfigurationRootsError, RESERVED_GROUP_ALL,
    },
    relay::{CredentialDestination, RelayError},
    runtime::{
        error::RuntimeError,
        paths::{RuntimeRootOverrides, RuntimeRoots},
    },
};

use super::{LOOK_LINES_MAXIMUM, LOOK_LINES_MINIMUM, RuntimeArguments};

/// Prints one line per session that failed to come up, so a partially started
/// bundle names its casualties in the command's own output rather than requiring
/// the operator to follow up with `list`.
///
/// Shared by both bring-up surfaces — `up`'s transition summary and the relay
/// host's startup summary — because a partial startup that reads one way on one
/// of them and another way on the other is the problem this exists to close.
/// `details` is the entry's structured detail; anything without a
/// `failed_sessions` array renders nothing.
pub(super) fn render_failed_sessions(bundle_name: &str, details: Option<&serde_json::Value>) {
    let Some(failed_sessions) = details
        .and_then(|details| details.get("failed_sessions"))
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    for failed_session in failed_sessions {
        let session_id = failed_session
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>");
        let reason = failed_session
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>");
        let cause = failed_session
            .get("details")
            .and_then(|details| details.get("cause"))
            .and_then(serde_json::Value::as_str);
        match cause {
            Some(cause) => {
                println!(
                    "  session={session_id} bundle={bundle_name} reason={reason} cause={cause}"
                )
            }
            None => println!("  session={session_id} bundle={bundle_name} reason={reason}"),
        }
    }
}

pub(super) fn parse_runtime_flag(
    arguments: &[String],
    index: &mut usize,
    runtime: &mut RuntimeArguments,
) -> Result<bool, RuntimeError> {
    match arguments[*index].as_str() {
        "--configuration-directory" => {
            // Repeatable: each occurrence appends one layer, and the layers are
            // searched in the order given, so the first occurrence is the
            // highest-precedence one.
            let value = take_value(arguments, index, "--configuration-directory")?;
            if value.is_empty() {
                return Err(RuntimeError::validation(
                    "validation_invalid_configuration_layers",
                    ConfigurationRootsError::EmptyElement {
                        position: runtime.configuration_layers.len(),
                    }
                    .to_string(),
                ));
            }
            runtime.configuration_layers.push(PathBuf::from(value));
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
        _ => Ok(false),
    }
}

/// Resolves runtime roots.
///
/// The association override file no longer participates: it lives *under* the
/// configuration root, so letting it select that root made the lookup circular.
/// Roots resolve first, and the association file is read from the resolved root.
pub(super) fn resolve_roots(runtime: &RuntimeArguments) -> Result<RuntimeRoots, RuntimeError> {
    RuntimeRoots::resolve(&RuntimeRootOverrides {
        configuration_layers: runtime.configuration_layers.clone(),
        state_root: runtime.state_root.clone(),
        inscriptions_root: runtime.inscriptions_root.clone(),
    })
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

/// Folds the mutually-exclusive `--output` / `--write-config` credential sink
/// flags into a relay `CredentialDestination`, rejecting the case where both are
/// supplied. Shared by `new peer` and `change psk`.
pub(super) fn resolve_credential_destination(
    output_path: Option<&str>,
    write_to_config: bool,
) -> Result<CredentialDestination, RuntimeError> {
    // Presence — not emptiness — drives mutual exclusion: a supplied `--output`
    // (even empty) conflicts with `--write-config`, and an empty path is
    // forwarded for the relay to reject as `validation_invalid_output_path`
    // rather than silently degrading to Response.
    match (output_path, write_to_config) {
        (Some(_), true) => Err(RuntimeError::validation(
            "validation_invalid_params",
            "--output and --write-config are mutually exclusive".to_string(),
        )),
        (Some(path), false) => Ok(CredentialDestination::Path {
            path: path.to_string(),
        }),
        (None, true) => Ok(CredentialDestination::Config),
        (None, false) => Ok(CredentialDestination::Response),
    }
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
    // Fold any config-diagnostic detail (file path + offending field) the relay
    // attached into the surfaced message so a config parse / unknown-field
    // failure reaching `agentmux up` names the file and field, not just a bare
    // summary. Other errors keep their plain message.
    let message = error.operator_message();
    if error.code.starts_with("validation_") || error.code == "authorization_forbidden" {
        return RuntimeError::validation(error.code, message);
    }
    // An internal relay code stays an IO status (not an actionable validation
    // code), but carry the code into the diagnostic so the real cause survives
    // instead of collapsing every internal failure into one opaque string.
    RuntimeError::io(
        message,
        std::io::Error::other(format!("relay error {}", error.code)),
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
