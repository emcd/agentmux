//! MCP bundle/session association discovery and override resolution.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::configuration::{
    BundleConfiguration, ConfigurationError, infer_sender_from_working_directory,
};

use super::error::RuntimeError;

const OVERRIDE_FILE_PATH: &str = ".auxiliary/configuration/agentmux/overrides/mcp.toml";

/// Git and workspace context used for association auto-discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub current_directory: PathBuf,
    pub workspace_root: PathBuf,
    pub git_top_level: Option<PathBuf>,
    pub git_common_dir: Option<PathBuf>,
}

impl WorkspaceContext {
    /// Discovers workspace context using current directory and optional Git
    /// metadata.
    ///
    /// # Errors
    ///
    /// Returns `RuntimeError` when current directory cannot be resolved.
    pub fn discover(current_directory: &Path) -> Result<Self, RuntimeError> {
        let current_directory = current_directory.to_path_buf();
        let git_top_level = run_git(
            current_directory.as_path(),
            &["rev-parse", "--show-toplevel"],
        )
        .map(PathBuf::from);
        let git_common_dir = run_git(
            current_directory.as_path(),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .or_else(|| {
            run_git(
                current_directory.as_path(),
                &["rev-parse", "--git-common-dir"],
            )
        })
        .map(PathBuf::from)
        .map(|path| normalize_path(&current_directory, &path));
        let workspace_root = git_top_level
            .clone()
            .unwrap_or_else(|| current_directory.clone());
        Ok(Self {
            current_directory,
            workspace_root,
            git_top_level,
            git_common_dir,
        })
    }

    /// Auto-discovers bundle name from Git common-dir parent basename,
    /// falling back to current-directory basename.
    ///
    /// # Errors
    ///
    /// Returns a validation error when name cannot be derived.
    pub fn auto_bundle_name(&self) -> Result<String, RuntimeError> {
        if let Some(common_dir) = self.git_common_dir.as_ref() {
            let parent = common_dir.parent().ok_or_else(|| {
                RuntimeError::validation(
                    "validation_unknown_bundle",
                    format!(
                        "cannot derive bundle name from git common-dir {}",
                        common_dir.display()
                    ),
                )
            })?;
            return basename(parent, "validation_unknown_bundle", "bundle");
        }
        basename(
            &self.current_directory,
            "validation_unknown_bundle",
            "bundle",
        )
    }

    /// Auto-discovers session name from worktree top-level basename, falling
    /// back to current-directory basename.
    ///
    /// # Errors
    ///
    /// Returns a validation error when name cannot be derived.
    pub fn auto_session_name(&self) -> Result<String, RuntimeError> {
        if let Some(top_level) = self.git_top_level.as_ref() {
            return basename(top_level, "validation_unknown_sender", "session");
        }
        basename(
            &self.current_directory,
            "validation_unknown_sender",
            "session",
        )
    }

    /// Resolves the repository root used for debug local state/config defaults.
    ///
    /// Uses the Git common-dir owner repository root when available (for
    /// example, for worktrees). Returns `None` when this cannot be resolved.
    #[must_use]
    pub fn debug_repository_root(&self) -> Option<PathBuf> {
        if let Some(common_dir) = self.git_common_dir.as_ref()
            && let Some(repository_root) = repository_root_from_git_common_dir(common_dir)
        {
            return Some(repository_root);
        }
        None
    }
}

/// CLI association hints provided by MCP startup arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpAssociationCli {
    pub bundle_name: Option<String>,
    pub session_name: Option<String>,
}

/// Local per-worktree association overrides.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct McpAssociationOverrides {
    #[serde(default)]
    pub bundle_name: Option<String>,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub config_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpAssociationOverrideFile {
    #[serde(default)]
    bundle_name: Option<String>,
    #[serde(default)]
    session_name: Option<String>,
    #[serde(default)]
    config_root: Option<PathBuf>,
}

/// Fully resolved MCP association identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAssociation {
    pub bundle_name: String,
    pub session_name: String,
}

/// Loads optional per-worktree MCP override file.
///
/// # Errors
///
/// Returns validation errors for malformed override file content.
pub fn load_local_mcp_overrides(
    workspace_root: &Path,
) -> Result<Option<McpAssociationOverrides>, RuntimeError> {
    let path = workspace_root.join(OVERRIDE_FILE_PATH);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|source| {
        RuntimeError::io(
            format!("read local MCP override file {}", path.display()),
            source,
        )
    })?;
    let parsed = toml::from_str::<McpAssociationOverrideFile>(&raw).map_err(|source| {
        RuntimeError::validation(
            "validation_invalid_arguments",
            format!(
                "malformed local MCP override file {}: {source}",
                path.display()
            ),
        )
    })?;
    Ok(Some(normalize_overrides(parsed, workspace_root)))
}

/// Resolves bundle and session identity with precedence:
/// CLI > local override > auto-discovery.
///
/// # Errors
///
/// Returns validation errors when identities cannot be derived.
pub fn resolve_association(
    cli: &McpAssociationCli,
    local_overrides: Option<&McpAssociationOverrides>,
    workspace: &WorkspaceContext,
) -> Result<ResolvedAssociation, RuntimeError> {
    let bundle_name = cli
        .bundle_name
        .clone()
        .or_else(|| local_overrides.and_then(|overrides| overrides.bundle_name.clone()))
        .and_then(normalize_string)
        .map(Ok)
        .unwrap_or_else(|| workspace.auto_bundle_name())?;
    let session_name = cli
        .session_name
        .clone()
        .or_else(|| local_overrides.and_then(|overrides| overrides.session_name.clone()))
        .and_then(normalize_string)
        .map(Ok)
        .unwrap_or_else(|| workspace.auto_session_name())?;
    Ok(ResolvedAssociation {
        bundle_name,
        session_name,
    })
}

/// Validates that resolved sender exists as bundle member.
///
/// # Errors
///
/// Returns `validation_unknown_sender` when sender is not a member.
pub fn validate_sender_session(
    bundle: &BundleConfiguration,
    session_name: &str,
) -> Result<String, RuntimeError> {
    if bundle
        .members
        .iter()
        .any(|member| member.id == session_name)
    {
        return Ok(session_name.to_string());
    }
    Err(RuntimeError::validation(
        "validation_unknown_sender",
        format!(
            "session '{}' is not configured in bundle '{}'",
            session_name, bundle.bundle_name
        ),
    ))
}

/// Resolves sender session from candidate name with working-directory fallback.
///
/// First tries direct session membership. If candidate is not configured,
/// attempts to infer sender from the current working directory by matching
/// bundle member `directory` paths.
///
/// # Errors
///
/// Returns `validation_unknown_sender` when no sender can be resolved or when
/// working-directory inference is ambiguous.
pub fn resolve_sender_session(
    bundle: &BundleConfiguration,
    candidate_session_name: &str,
    working_directory: &Path,
) -> Result<String, RuntimeError> {
    if let Ok(session_name) = validate_sender_session(bundle, candidate_session_name) {
        return Ok(session_name);
    }

    let inferred = infer_sender_from_working_directory(bundle, working_directory)
        .map_err(map_sender_inference_error)?;
    if let Some(inferred) = inferred {
        return Ok(inferred);
    }

    Err(RuntimeError::validation(
        "validation_unknown_sender",
        format!(
            "session '{}' is not configured in bundle '{}' and working directory '{}' did not match any configured session directory",
            candidate_session_name,
            bundle.bundle_name,
            working_directory.display()
        ),
    ))
}

fn run_git(directory: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_string(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn normalize_overrides(
    parsed: McpAssociationOverrideFile,
    workspace_root: &Path,
) -> McpAssociationOverrides {
    let config_root = parsed.config_root.and_then(|path| {
        if path.as_os_str().is_empty() {
            return None;
        }
        let normalized = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        Some(normalized)
    });
    McpAssociationOverrides {
        bundle_name: parsed.bundle_name.and_then(normalize_string),
        session_name: parsed.session_name.and_then(normalize_string),
        config_root,
    }
}

fn repository_root_from_git_common_dir(common_dir: &Path) -> Option<PathBuf> {
    let mut cursor = Some(common_dir);
    while let Some(path) = cursor {
        if path.file_name().is_some_and(|name| name == ".git") {
            return path.parent().map(Path::to_path_buf);
        }
        cursor = path.parent();
    }
    None
}

fn normalize_path(current_directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    current_directory.join(path)
}

fn basename(path: &Path, code: &str, noun: &str) -> Result<String, RuntimeError> {
    let value = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(normalize_str);
    if let Some(value) = value {
        return Ok(value.to_string());
    }
    Err(RuntimeError::validation(
        code,
        format!("cannot derive {noun} name from {}", path.display()),
    ))
}

fn normalize_string(value: String) -> Option<String> {
    normalize_str(value.as_str()).map(ToString::to_string)
}

fn normalize_str(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}

fn map_sender_inference_error(source: ConfigurationError) -> RuntimeError {
    match source {
        ConfigurationError::AmbiguousSender {
            working_directory,
            matches,
        } => RuntimeError::validation(
            "validation_unknown_sender",
            format!(
                "working directory '{}' matched multiple configured sessions: {}",
                working_directory.display(),
                matches.join(", ")
            ),
        ),
        other => RuntimeError::validation(
            "validation_unknown_sender",
            format!("failed to infer sender from working directory: {other}"),
        ),
    }
}
