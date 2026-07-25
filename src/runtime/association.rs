//! MCP bundle/session association discovery and override resolution.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::configuration::{
    BUNDLE_ENVIRONMENT_VARIABLE, BundleConfiguration, ConfigurationError,
    SESSION_ENVIRONMENT_VARIABLE, effective_configuration_path,
    infer_sender_from_working_directory,
};

use super::error::RuntimeError;

/// Logical artifact holding per-tree association overrides. Resolved through
/// the configuration overlay like every other configuration file, so an overlay
/// copy shadows a base copy.
const ASSOCIATION_FILE: &str = "mcp.toml";

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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpAssociationOverrideFile {
    #[serde(default)]
    bundle_name: Option<String>,
    #[serde(default)]
    session_name: Option<String>,
}

/// Loads optional per-worktree MCP override file.
///
/// # Errors
///
/// Returns validation errors for malformed override file content.
pub fn load_local_mcp_overrides(
    configuration_root: &Path,
) -> Result<Option<McpAssociationOverrides>, RuntimeError> {
    let path = effective_configuration_path(configuration_root, ASSOCIATION_FILE);
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
    Ok(Some(McpAssociationOverrides {
        bundle_name: parsed.bundle_name.and_then(normalize_string),
        session_name: parsed.session_name.and_then(normalize_string),
    }))
}

/// Association identities carried by the injected bring-up environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpAssociationEnvironment {
    pub bundle_name: Option<String>,
    pub session_name: Option<String>,
}

impl McpAssociationEnvironment {
    /// Reads the injected context from the process environment.
    ///
    /// Blank values normalize to absent here, at the single point the
    /// environment enters the process, so no consumer has to decide separately
    /// whether a blank value counts as present.
    #[must_use]
    pub fn from_process_environment() -> Self {
        Self {
            bundle_name: std::env::var(BUNDLE_ENVIRONMENT_VARIABLE)
                .ok()
                .and_then(normalize_string),
            session_name: std::env::var(SESSION_ENVIRONMENT_VARIABLE)
                .ok()
                .and_then(normalize_string),
        }
    }
}

/// Bundle and session identities as far as they resolve, each independently
/// optional.
///
/// Absence is a recorded condition rather than a failure. A relay-wide server
/// with no bundle is legitimate, and a misconfigured one should report its cause
/// where the agent will see it rather than erase its tool surface at startup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssociationCandidates {
    pub bundle_name: Option<String>,
    pub session_name: Option<String>,
}

/// Resolves association identity by precedence, each tier ranked by how
/// deployment-specific its source is.
///
/// Bundle: `--bundle` > injected environment > overlay file > `--default-bundle`.
/// Session: `--session-name` > injected environment > overlay file, with the
/// working-directory match against declared member directories applied later by
/// the caller, once the bundle configuration is loaded.
///
/// `--default-bundle` sits *below* the injected environment so generated client
/// configuration can seed a bundle without impersonating invocation intent.
/// `--bundle` sits above it and still asserts that intent.
#[must_use]
pub fn resolve_association(
    cli: &McpAssociationCli,
    environment: &McpAssociationEnvironment,
    local_overrides: Option<&McpAssociationOverrides>,
    default_bundle: Option<&str>,
) -> AssociationCandidates {
    let bundle_name = cli
        .bundle_name
        .clone()
        .and_then(normalize_string)
        .or_else(|| environment.bundle_name.clone())
        .or_else(|| local_overrides.and_then(|overrides| overrides.bundle_name.clone()))
        .or_else(|| {
            default_bundle
                .map(ToString::to_string)
                .and_then(normalize_string)
        });
    let session_name = cli
        .session_name
        .clone()
        .and_then(normalize_string)
        .or_else(|| environment.session_name.clone())
        .or_else(|| local_overrides.and_then(|overrides| overrides.session_name.clone()));
    AssociationCandidates {
        bundle_name,
        session_name,
    }
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
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_string(String::from_utf8_lossy(&output.stdout).into_owned())
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
