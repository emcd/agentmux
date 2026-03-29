//! TUI session configuration discovery and precedence resolution.

use std::path::{Path, PathBuf};

use crate::configuration::{
    ConfigurationError, TuiConfiguration, load_policy_ids, load_tui_configuration,
    load_tui_configuration_file,
};

use super::error::RuntimeError;

const OVERRIDE_FILE_PATH: &str = ".auxiliary/configuration/agentmux/overrides/tui.toml";

/// Resolved TUI session identity for CLI/TUI operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTuiSession {
    pub bundle_name: String,
    pub session_selector: String,
    pub session_id: String,
    pub session_name: Option<String>,
    pub policy_id: String,
}

/// Loads active TUI configuration with debug/testing override precedence.
///
/// # Errors
///
/// Returns `RuntimeError` when configuration files are malformed.
pub fn load_active_tui_configuration(
    configuration_root: &Path,
    workspace_root: &Path,
) -> Result<Option<TuiConfiguration>, RuntimeError> {
    if let Some(override_path) = local_override_path(workspace_root) {
        return load_tui_configuration_file(&override_path).map_err(|source| {
            map_configuration_error(source, "load local TUI override configuration")
        });
    }
    load_tui_configuration(configuration_root)
        .map_err(|source| map_configuration_error(source, "load TUI configuration"))
}

/// Resolves TUI bundle/session identity with deterministic precedence.
///
/// Resolution order:
/// 1. explicit `--bundle` and `--session`
/// 2. `default-bundle` and `default-session` from active TUI configuration
/// 3. fail-fast validation errors
///
/// # Errors
///
/// Returns validation errors for missing selectors, unknown sessions, and
/// unknown policy references.
pub fn resolve_tui_session_identity(
    configuration_root: &Path,
    workspace_root: &Path,
    explicit_bundle: Option<&str>,
    explicit_session: Option<&str>,
) -> Result<ResolvedTuiSession, RuntimeError> {
    let configuration = load_active_tui_configuration(configuration_root, workspace_root)?;
    let bundle_name = resolve_bundle_name(configuration.as_ref(), explicit_bundle)?;
    let selector = resolve_session_selector(configuration.as_ref(), explicit_session)?;
    let selected = resolve_selected_session(configuration.as_ref(), selector.as_str())?;
    validate_sender_shape(selected.id.as_str())?;
    validate_selected_policy(configuration_root, selected.policy_id.as_str())?;
    Ok(ResolvedTuiSession {
        bundle_name,
        session_selector: selector,
        session_id: selected.id.clone(),
        session_name: selected.name.clone(),
        policy_id: selected.policy_id.clone(),
    })
}

fn resolve_bundle_name(
    configuration: Option<&TuiConfiguration>,
    explicit_bundle: Option<&str>,
) -> Result<String, RuntimeError> {
    if let Some(bundle_name) = explicit_bundle.and_then(normalize) {
        return Ok(bundle_name.to_string());
    }
    if let Some(bundle_name) = configuration
        .and_then(|configuration| configuration.default_bundle.as_deref())
        .and_then(normalize)
    {
        return Ok(bundle_name.to_string());
    }
    Err(RuntimeError::validation(
        "validation_unknown_bundle",
        "bundle is required via --bundle or tui.toml default-bundle".to_string(),
    ))
}

fn resolve_session_selector(
    configuration: Option<&TuiConfiguration>,
    explicit_session: Option<&str>,
) -> Result<String, RuntimeError> {
    if let Some(session) = explicit_session.and_then(normalize) {
        return Ok(session.to_string());
    }
    if let Some(session) = configuration
        .and_then(|configuration| configuration.default_session.as_deref())
        .and_then(normalize)
    {
        return Ok(session.to_string());
    }
    Err(RuntimeError::validation(
        "validation_unknown_session",
        "session is required via --session or tui.toml default-session".to_string(),
    ))
}

fn resolve_selected_session<'a>(
    configuration: Option<&'a TuiConfiguration>,
    selector: &str,
) -> Result<&'a crate::configuration::TuiSession, RuntimeError> {
    let Some(configuration) = configuration else {
        return Err(RuntimeError::validation(
            "validation_unknown_session",
            format!("session '{}' is not configured in tui.toml", selector),
        ));
    };
    configuration.session_by_id(selector).ok_or_else(|| {
        RuntimeError::validation(
            "validation_unknown_session",
            format!("session '{}' is not configured in tui.toml", selector),
        )
    })
}

fn validate_selected_policy(
    configuration_root: &Path,
    policy_id: &str,
) -> Result<(), RuntimeError> {
    let policy_ids = load_policy_ids(configuration_root)
        .map_err(|source| map_configuration_error(source, "load policy presets"))?;
    if policy_ids.contains(policy_id) {
        return Ok(());
    }
    Err(RuntimeError::validation(
        "validation_unknown_policy",
        format!(
            "session policy '{}' is not configured in policies.toml",
            policy_id
        ),
    ))
}

fn validate_sender_shape(session_id: &str) -> Result<(), RuntimeError> {
    let Some(first) = session_id.chars().next() else {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            "session id is empty".to_string(),
        ));
    };
    if !first.is_ascii_alphabetic() {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            format!(
                "session id '{}' must start with an ASCII alphabetic character",
                session_id
            ),
        ));
    }
    if !session_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(RuntimeError::validation(
            "validation_unknown_sender",
            format!(
                "session id '{}' may only contain ASCII alphanumeric characters, '-' or '_'",
                session_id
            ),
        ));
    }
    Ok(())
}

fn local_override_path(workspace_root: &Path) -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let path = workspace_root.join(OVERRIDE_FILE_PATH);
    if path.exists() {
        return Some(path);
    }
    None
}

fn map_configuration_error(source: ConfigurationError, context: &str) -> RuntimeError {
    match source {
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!("{context} {}: {}", path.display(), message),
        ),
        ConfigurationError::Io { context, source } => RuntimeError::io(context, source),
        other => RuntimeError::validation(
            "validation_invalid_arguments",
            format!("{context}: {other}"),
        ),
    }
}

fn normalize(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}
