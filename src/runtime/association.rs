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

/// Which precedence tier supplied an association identity.
///
/// Recorded so an operator can tell the tier they intended from the tier that
/// actually won. The injected environment is the one tier whose delivery depends
/// on an agent harness passing it through to a grandchild process; a resolution
/// that silently fell to a lower tier is otherwise indistinguishable from one
/// that never needed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssociationSource {
    CommandLine,
    Environment,
    Overlay,
    DefaultBundle,
    WorkingDirectory,
}

impl AssociationSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandLine => "command-line",
            Self::Environment => "environment",
            Self::Overlay => "overlay",
            Self::DefaultBundle => "default-bundle",
            Self::WorkingDirectory => "working-directory",
        }
    }
}

/// An association identity together with the tier which supplied it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssociationCandidate {
    pub value: String,
    pub source: AssociationSource,
}

/// Bundle and session identities as far as they resolve, each independently
/// optional.
///
/// Absence is a recorded condition rather than a failure. A relay-wide server
/// with no bundle is legitimate, and a misconfigured one should report its cause
/// where the agent will see it rather than erase its tool surface at startup.
///
/// Absence is *not* interchangeable with an unresolvable value: presence carries
/// operator intent, so a supplied identity which does not resolve is a fault
/// rather than an invitation to try a lower tier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssociationCandidates {
    pub bundle: Option<AssociationCandidate>,
    pub session: Option<AssociationCandidate>,
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
    let bundle = candidate(cli.bundle_name.clone(), AssociationSource::CommandLine)
        .or_else(|| {
            candidate(
                environment.bundle_name.clone(),
                AssociationSource::Environment,
            )
        })
        .or_else(|| {
            candidate(
                local_overrides.and_then(|overrides| overrides.bundle_name.clone()),
                AssociationSource::Overlay,
            )
        })
        .or_else(|| {
            candidate(
                default_bundle.map(ToString::to_string),
                AssociationSource::DefaultBundle,
            )
        });
    let session = candidate(cli.session_name.clone(), AssociationSource::CommandLine)
        .or_else(|| {
            candidate(
                environment.session_name.clone(),
                AssociationSource::Environment,
            )
        })
        .or_else(|| {
            candidate(
                local_overrides.and_then(|overrides| overrides.session_name.clone()),
                AssociationSource::Overlay,
            )
        });
    AssociationCandidates { bundle, session }
}

/// Pairs a tier's value with that tier, dropping it when the tier supplied
/// nothing. Blank normalizes to absent so a tier cannot claim a turn with an
/// empty string.
fn candidate(value: Option<String>, source: AssociationSource) -> Option<AssociationCandidate> {
    value
        .and_then(normalize_string)
        .map(|value| AssociationCandidate { value, source })
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

/// Resolves the sender session from a candidate, or from the working directory
/// when no tier supplied one.
///
/// A supplied candidate is validated against bundle membership and nothing else.
/// The working-directory match is the tier that applies in the *absence* of a
/// candidate, so letting an unresolvable candidate fall into it would let a typo
/// authenticate as whichever member happens to own the current directory --
/// identity resolved by inference where it was in fact declared.
///
/// `Ok(None)` reports a legitimately unassociated server: nothing was supplied
/// and the working directory matched no member.
///
/// # Errors
///
/// Returns `validation_unknown_sender` when a supplied candidate names no
/// member, or when the working directory matches more than one.
pub fn resolve_sender_session(
    bundle: &BundleConfiguration,
    candidate_session_name: Option<&str>,
    working_directory: &Path,
) -> Result<Option<String>, RuntimeError> {
    if let Some(candidate_session_name) = candidate_session_name {
        return validate_sender_session(bundle, candidate_session_name).map(Some);
    }
    infer_sender_from_working_directory(bundle, working_directory)
        .map_err(map_sender_inference_error)
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
