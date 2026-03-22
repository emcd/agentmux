//! Runtime path resolution for configuration, state, and bundle sockets.

use std::{
    env, fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::error::RuntimeError;

const APPLICATION_DIRECTORY: &str = "agentmux";
const CONFIGURATION_DIRECTORY_DEFAULT: &str = ".config";
const STATE_DIRECTORY_DEFAULT: &str = ".local/state";
const INSCRIPTIONS_DIRECTORY_DEFAULT: &str = "inscriptions";
const BUNDLES_DIRECTORY: &str = "bundles";
const RELAY_SOCKET_FILE: &str = "relay.sock";
const TMUX_SOCKET_FILE: &str = "tmux.sock";
const RELAY_LOCK_FILE: &str = "relay.lock";
const RELAY_SPAWN_LOCK_FILE: &str = "relay.spawn.lock";
const DIRECTORY_MODE_OWNER_ONLY: u32 = 0o700;

/// Optional overrides for runtime root resolution.
#[derive(Clone, Debug, Default)]
pub struct RuntimeRootOverrides {
    pub configuration_root: Option<PathBuf>,
    pub state_root: Option<PathBuf>,
    pub inscriptions_root: Option<PathBuf>,
    pub repository_root: Option<PathBuf>,
}

/// Resolved application roots for configuration and state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRoots {
    pub configuration_root: PathBuf,
    pub state_root: PathBuf,
    pub inscriptions_root: PathBuf,
}

impl RuntimeRoots {
    /// Resolves runtime roots from overrides, environment, and defaults.
    ///
    /// # Errors
    ///
    /// Returns `RuntimeError::HomeDirectoryUnavailable` if `HOME` is not
    /// available and no explicit or XDG paths are configured.
    pub fn resolve(overrides: &RuntimeRootOverrides) -> Result<Self, RuntimeError> {
        let configuration_root = resolve_configuration_root(overrides)?;
        let state_root = resolve_state_root(overrides)?;
        let inscriptions_root = resolve_inscriptions_root(overrides, &state_root);
        Ok(Self {
            configuration_root,
            state_root,
            inscriptions_root,
        })
    }
}

/// Resolved per-bundle runtime paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRuntimePaths {
    pub state_root: PathBuf,
    pub bundle_name: String,
    pub runtime_directory: PathBuf,
    pub tmux_socket: PathBuf,
    pub relay_socket: PathBuf,
    pub relay_lock_file: PathBuf,
    pub relay_spawn_lock_file: PathBuf,
}

impl BundleRuntimePaths {
    /// Resolves all runtime paths for a bundle.
    ///
    /// # Errors
    ///
    /// Returns `RuntimeError::InvalidBundleName` when bundle name contains
    /// unsupported characters.
    pub fn resolve(state_root: &Path, bundle_name: &str) -> Result<Self, RuntimeError> {
        validate_bundle_name(bundle_name)?;
        let runtime_directory = state_root.join(BUNDLES_DIRECTORY).join(bundle_name);
        Ok(Self {
            state_root: state_root.to_path_buf(),
            bundle_name: bundle_name.to_string(),
            tmux_socket: runtime_directory.join(TMUX_SOCKET_FILE),
            relay_socket: runtime_directory.join(RELAY_SOCKET_FILE),
            relay_lock_file: runtime_directory.join(RELAY_LOCK_FILE),
            relay_spawn_lock_file: runtime_directory.join(RELAY_SPAWN_LOCK_FILE),
            runtime_directory,
        })
    }
}

/// Resolves the debug repository-local state root.
pub fn debug_repository_state_root(repository_root: &Path) -> PathBuf {
    repository_root
        .join(".auxiliary/state")
        .join(APPLICATION_DIRECTORY)
}

/// Resolves the debug repository-local configuration root.
pub fn debug_repository_configuration_root(repository_root: &Path) -> PathBuf {
    repository_root
        .join(".auxiliary/configuration")
        .join(APPLICATION_DIRECTORY)
}

/// Resolves the debug repository-local inscriptions root.
pub fn debug_repository_inscriptions_root(repository_root: &Path) -> PathBuf {
    repository_root
        .join(".auxiliary/inscriptions")
        .join(APPLICATION_DIRECTORY)
}

/// Ensures the bundle runtime directory exists with owner-only permissions.
///
/// # Errors
///
/// Returns a security error when an existing path is owned by another user.
pub fn ensure_bundle_runtime_directory(paths: &BundleRuntimePaths) -> Result<(), RuntimeError> {
    ensure_directory_secure(&paths.runtime_directory)
}

/// Verifies that an existing filesystem artifact is current-user owned.
///
/// # Errors
///
/// Returns `RuntimeError::SecurityForeignOwned` for foreign-owned artifacts.
pub fn ensure_existing_artifact_is_owned(path: &Path) -> Result<(), RuntimeError> {
    if !path.exists() {
        return Ok(());
    }
    ensure_current_user_owns(path)
}

fn resolve_configuration_root(overrides: &RuntimeRootOverrides) -> Result<PathBuf, RuntimeError> {
    if let Some(path) = overrides.configuration_root.clone() {
        return Ok(path);
    }
    if cfg!(debug_assertions)
        && let Some(repository_root) = overrides.repository_root.as_ref()
    {
        let debug_root = debug_repository_configuration_root(repository_root);
        if debug_root.is_dir() {
            return Ok(debug_root);
        }
    }
    if let Some(path) = env_directory("XDG_CONFIG_HOME") {
        return Ok(path.join(APPLICATION_DIRECTORY));
    }
    let home_directory = resolve_home_directory()?;
    Ok(configuration_root_from_sources(None, &home_directory))
}

fn resolve_state_root(overrides: &RuntimeRootOverrides) -> Result<PathBuf, RuntimeError> {
    if let Some(path) = overrides.state_root.clone() {
        return Ok(path);
    }
    if cfg!(debug_assertions)
        && let Some(repository_root) = overrides.repository_root.as_ref()
    {
        return Ok(debug_repository_state_root(repository_root));
    }
    if let Some(path) = env_directory("XDG_STATE_HOME") {
        return Ok(path.join(APPLICATION_DIRECTORY));
    }
    let home_directory = resolve_home_directory()?;
    Ok(state_root_from_sources(None, &home_directory))
}

fn resolve_inscriptions_root(overrides: &RuntimeRootOverrides, state_root: &Path) -> PathBuf {
    if let Some(path) = overrides.inscriptions_root.clone() {
        return path;
    }
    if cfg!(debug_assertions)
        && let Some(repository_root) = overrides.repository_root.as_ref()
    {
        return debug_repository_inscriptions_root(repository_root);
    }
    state_root.join(INSCRIPTIONS_DIRECTORY_DEFAULT)
}

fn resolve_home_directory() -> Result<PathBuf, RuntimeError> {
    let Some(home) = env_directory("HOME") else {
        return Err(RuntimeError::HomeDirectoryUnavailable);
    };
    Ok(home)
}

fn env_directory(variable_name: &str) -> Option<PathBuf> {
    env::var(variable_name).ok().and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        Some(PathBuf::from(value))
    })
}

fn validate_bundle_name(bundle_name: &str) -> Result<(), RuntimeError> {
    let valid = !bundle_name.is_empty()
        && bundle_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });
    if valid {
        return Ok(());
    }
    Err(RuntimeError::InvalidBundleName {
        bundle_name: bundle_name.to_string(),
    })
}

fn ensure_directory_secure(path: &Path) -> Result<(), RuntimeError> {
    if !path.exists() {
        fs::create_dir_all(path).map_err(|source| {
            RuntimeError::io(
                format!("create runtime directory {}", path.display()),
                source,
            )
        })?;
    }
    if !path.is_dir() {
        return Err(RuntimeError::io(
            format!("runtime path is not a directory {}", path.display()),
            std::io::Error::other("not a directory"),
        ));
    }
    ensure_current_user_owns(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE_OWNER_ONLY))
        .map_err(|source| RuntimeError::io(format!("set mode 0700 on {}", path.display()), source))
}

fn ensure_current_user_owns(path: &Path) -> Result<(), RuntimeError> {
    let metadata = fs::metadata(path)
        .map_err(|source| RuntimeError::io(format!("read metadata {}", path.display()), source))?;
    let expected_uid = current_effective_uid();
    let actual_uid = metadata.uid();
    if actual_uid == expected_uid {
        return Ok(());
    }
    Err(RuntimeError::SecurityForeignOwned {
        path: path.to_path_buf(),
        expected_uid,
        actual_uid,
    })
}

fn current_effective_uid() -> u32 {
    unsafe { libc::geteuid() as u32 }
}

fn configuration_root_from_sources(
    xdg_configuration_home: Option<&Path>,
    home_directory: &Path,
) -> PathBuf {
    if let Some(path) = xdg_configuration_home {
        return path.join(APPLICATION_DIRECTORY);
    }
    home_directory
        .join(CONFIGURATION_DIRECTORY_DEFAULT)
        .join(APPLICATION_DIRECTORY)
}

fn state_root_from_sources(xdg_state_home: Option<&Path>, home_directory: &Path) -> PathBuf {
    if let Some(path) = xdg_state_home {
        return path.join(APPLICATION_DIRECTORY);
    }
    home_directory
        .join(STATE_DIRECTORY_DEFAULT)
        .join(APPLICATION_DIRECTORY)
}
