//! TUI sender configuration discovery and precedence resolution.

use std::{fs, path::Path};

use serde::Deserialize;

use crate::{
    configuration::BundleConfiguration,
    runtime::association::{resolve_sender_session, validate_sender_session},
};

use super::error::RuntimeError;

const OVERRIDE_FILE_PATH: &str = ".auxiliary/configuration/agentmux/overrides/tui.toml";
const CONFIG_FILE_NAME: &str = "tui.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TuiSenderFile {
    sender: String,
}

/// Loads optional debug/testing override sender from local overrides.
///
/// The override file is only active in debug/testing builds.
///
/// # Errors
///
/// Returns a validation error when file content is malformed.
pub fn load_local_tui_override_sender(
    workspace_root: &Path,
) -> Result<Option<String>, RuntimeError> {
    if !cfg!(debug_assertions) {
        return Ok(None);
    }
    load_sender_file(
        workspace_root.join(OVERRIDE_FILE_PATH).as_path(),
        "local TUI override sender file",
    )
}

/// Loads optional normal TUI sender default from configuration root.
///
/// # Errors
///
/// Returns a validation error when file content is malformed.
pub fn load_tui_configuration_sender(
    configuration_root: &Path,
) -> Result<Option<String>, RuntimeError> {
    load_sender_file(
        configuration_root.join(CONFIG_FILE_NAME).as_path(),
        "TUI sender config file",
    )
}

/// Resolves TUI sender session with deterministic precedence.
///
/// Precedence order:
/// 1. CLI sender
/// 2. local override sender
/// 3. configuration sender
/// 4. association fallback sender
///
/// # Errors
///
/// Returns `validation_unknown_sender` when resolved sender is not a known
/// bundle member.
pub fn resolve_tui_sender_session(
    bundle: &BundleConfiguration,
    working_directory: &Path,
    association_sender: &str,
    cli_sender: Option<&str>,
    override_sender: Option<&str>,
    configuration_sender: Option<&str>,
) -> Result<String, RuntimeError> {
    if let Some(sender) = cli_sender {
        return validate_sender_session(bundle, sender);
    }
    if let Some(sender) = override_sender {
        return validate_sender_session(bundle, sender);
    }
    if let Some(sender) = configuration_sender {
        return validate_sender_session(bundle, sender);
    }
    resolve_sender_session(bundle, association_sender, working_directory)
}

fn load_sender_file(path: &Path, label: &str) -> Result<Option<String>, RuntimeError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|source| RuntimeError::io(format!("read {} {}", label, path.display()), source))?;
    let parsed = toml::from_str::<TuiSenderFile>(&raw).map_err(|source| {
        RuntimeError::validation(
            "validation_invalid_arguments",
            format!("malformed {} {}: {source}", label, path.display()),
        )
    })?;
    normalize_sender(parsed.sender, path)
}

fn normalize_sender(value: String, path: &Path) -> Result<Option<String>, RuntimeError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(RuntimeError::validation(
            "validation_invalid_arguments",
            format!(
                "malformed TUI sender config file {}: sender must be non-empty",
                path.display()
            ),
        ));
    }
    Ok(Some(normalized.to_string()))
}
