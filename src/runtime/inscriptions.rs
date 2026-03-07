//! Process-local structured inscription logging.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process,
    sync::OnceLock,
};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use super::error::RuntimeError;

static PROCESS_INSCRIPTIONS_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Resolves relay inscription file path for one bundle.
#[must_use]
pub fn relay_inscriptions_path(inscriptions_root: &Path, bundle_name: &str) -> PathBuf {
    inscriptions_root
        .join("bundles")
        .join(safe_segment(bundle_name))
        .join("relay.log")
}

/// Resolves MCP inscription file path for one bundle/session process.
#[must_use]
pub fn mcp_inscriptions_path(
    inscriptions_root: &Path,
    bundle_name: &str,
    session_name: &str,
) -> PathBuf {
    inscriptions_root
        .join("bundles")
        .join(safe_segment(bundle_name))
        .join("sessions")
        .join(safe_segment(session_name))
        .join("mcp.log")
}

/// Configures process-local inscription sink path.
///
/// # Errors
///
/// Returns `RuntimeError` when parent directory cannot be created or when a
/// conflicting path is already configured.
pub fn configure_process_inscriptions(path: &Path) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| RuntimeError::io(format!("create {}", parent.display()), source))?;
    }
    if let Some(existing) = PROCESS_INSCRIPTIONS_PATH.get() {
        if existing == path {
            return Ok(());
        }
        return Err(RuntimeError::InvalidArgument {
            argument: "--inscriptions-directory".to_string(),
            message: format!(
                "conflicting inscriptions path: {} is already configured",
                existing.display()
            ),
        });
    }
    PROCESS_INSCRIPTIONS_PATH
        .set(path.to_path_buf())
        .map_err(|_| RuntimeError::InvalidArgument {
            argument: "--inscriptions-directory".to_string(),
            message: "failed to configure inscriptions path".to_string(),
        })
}

/// Emits one structured inscription line to the configured sink.
pub fn emit_inscription(event: &str, details: &Value) {
    let Some(path) = PROCESS_INSCRIPTIONS_PATH.get() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let line = json!({
        "timestamp": timestamp,
        "pid": process::id(),
        "event": event,
        "details": details,
    })
    .to_string();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn safe_segment(value: &str) -> String {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            normalized.push(character);
        } else {
            normalized.push('_');
        }
    }
    if normalized.is_empty() {
        normalized.push_str("unknown");
    }
    normalized
}
