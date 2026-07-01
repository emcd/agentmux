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

/// Resolves the relay inscription file path. One log per relay instance,
/// rooted at the inscriptions directory.
#[must_use]
pub fn relay_inscriptions_path(inscriptions_root: &Path) -> PathBuf {
    inscriptions_root.join("relay.log")
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

/// Resolves MCP inscription file path for an unassociated MCP process.
#[must_use]
pub fn mcp_unassociated_inscriptions_path(inscriptions_root: &Path) -> PathBuf {
    inscriptions_root.join("mcp").join("unassociated.log")
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
    append_inscription_record(path, event, details);
}

/// Appends one structured inscription record (timestamped JSON object plus a
/// trailing newline) to `path` as a single atomic write under `O_APPEND`.
///
/// Separated from the process-global [`emit_inscription`] sink so the append
/// behavior can be exercised directly against an explicit path; `emit_inscription`
/// delegates here with the configured path.
///
/// Inscriptions are emitted concurrently from many tasks with no shared lock,
/// relying on `O_APPEND` to append each write atomically. The record is built as a
/// single string (content plus its trailing newline) and committed with one
/// `write_all`: a `writeln!` would issue the content and the newline as two
/// separate writes, letting concurrent emitters interleave one record's content
/// with another's newline and corrupt both lines into non-JSON that downstream
/// readers drop. One write of the complete record keeps each line intact.
pub fn append_inscription_record(path: &Path, event: &str, details: &Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let mut record = json!({
        "timestamp": timestamp,
        "pid": process::id(),
        "event": event,
        "details": details,
    })
    .to_string();
    record.push('\n');
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(record.as_bytes());
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

/// Environment variable that enables transport-side delivery diagnostic
/// inscriptions (e.g. `delivery_prime_timeout`). Without this gate set to a
/// truthy value, [`emit_delivery_diagnostic`] is a no-op so production runs
/// do not write per-flush diagnostics by default. Originally surfaced on
/// the tmux side as `AGENTMUX_RELAY_DELIVERY_DIAGNOSTICS`; the canonical
/// name is the same string whether the caller sets it from a tmux-driven
/// or ACP-driven test (no `*_acp_*` / `*_tmux_*` split — the env var is
/// transport-neutral).
pub const DELIVERY_DIAGNOSTICS_ENVVAR: &str = "AGENTMUX_RELAY_DELIVERY_DIAGNOSTICS";

/// Emits one transport-side delivery diagnostic inscription line under the
/// `relay.<event>` namespace. Gated by [`DELIVERY_DIAGNOSTICS_ENVVAR`]; when
/// the env var is unset or not a truthy value, the call is a no-op. Used by
/// the Tmux and ACP transports' prime-timeout / wedge-classifier paths.
pub fn emit_delivery_diagnostic(event: &str, details: &serde_json::Value) {
    if !delivery_diagnostics_enabled() {
        return;
    }
    emit_inscription(format!("relay.{event}").as_str(), details);
}

fn delivery_diagnostics_enabled() -> bool {
    std::env::var(DELIVERY_DIAGNOSTICS_ENVVAR)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}
