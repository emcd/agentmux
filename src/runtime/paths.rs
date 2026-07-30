//! Runtime path resolution for configuration, state, and bundle sockets.

use std::{
    env, fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use crate::configuration::{
    ConfigurationRoots, ConfigurationRootsError, STATE_DIRECTORY_ENVIRONMENT_VARIABLE,
};

use super::error::RuntimeError;

const APPLICATION_DIRECTORY: &str = "agentmux";
/// Environment tier of configuration-layer resolution. Ranked below the CLI
/// flag, and like the flag it replaces the layer list outright. Carries a
/// separator-delimited list, so a path containing the separator is
/// unrepresentable here and needs the repeatable flag.
const CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE: &str = "AGENTMUX_CONFIGURATION_DIRECTORY";
const CONFIGURATION_DIRECTORY_DEFAULT: &str = ".config";
const STATE_DIRECTORY_DEFAULT: &str = ".local/state";
const INSCRIPTIONS_DIRECTORY_DEFAULT: &str = "inscriptions";
const BUNDLES_DIRECTORY: &str = "bundles";
const SESSIONS_DIRECTORY: &str = "sessions";
const PEERS_DIRECTORY: &str = "peers";
const IDENTITY_DIRECTORY: &str = "identity";
const IDENTITY_PSK_FILE: &str = "identity.psk";
const PRINCIPAL_STORE_FILE: &str = "principals.json";
const RELAY_SOCKET_FILE: &str = "relay.sock";
const TMUX_SOCKET_FILE: &str = "tmux.sock";
const RELAY_LOCK_FILE: &str = "relay.lock";
const RELAY_SPAWN_LOCK_FILE: &str = "relay.spawn.lock";
const RELAY_READY_SENTINEL_FILE: &str = "relay.ready";
const DIRECTORY_MODE_OWNER_ONLY: u32 = 0o700;
const CREDENTIAL_FILE_MODE_OWNER_ONLY: u32 = 0o600;

/// Optional overrides for runtime root resolution.
#[derive(Clone, Debug, Default)]
pub struct RuntimeRootOverrides {
    /// Configuration layers in list order, empty when none were supplied. The
    /// list replaces the tier stack rather than extending it, so a supplied
    /// list is closed and no unsupplied root is consulted for any file.
    pub configuration_layers: Vec<PathBuf>,
    pub state_root: Option<PathBuf>,
    pub inscriptions_root: Option<PathBuf>,
}

/// Tier which supplied the configuration root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigurationRootSource {
    /// `--configuration-directory`.
    CommandLine,
    /// `AGENTMUX_CONFIGURATION_DIRECTORY`.
    Environment,
    /// `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`.
    Default,
}

impl ConfigurationRootSource {
    /// Reports whether a root from this tier may be scaffolded with starter
    /// configuration.
    ///
    /// Only the default tier may. Scaffolding a root the operator named would
    /// turn naming the wrong one into a fresh, empty, apparently-working
    /// deployment instead of an error.
    #[must_use]
    pub fn permits_hydration(self) -> bool {
        matches!(self, Self::Default)
    }
}

/// Resolved application roots for configuration and state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRoots {
    pub configuration_roots: ConfigurationRoots,
    pub state_root: PathBuf,
    pub inscriptions_root: PathBuf,
    pub configuration_root_source: ConfigurationRootSource,
}

impl RuntimeRoots {
    /// Resolves runtime roots from overrides, environment, and defaults.
    ///
    /// # Errors
    ///
    /// Returns `RuntimeError::HomeDirectoryUnavailable` if `HOME` is not
    /// available and no explicit or XDG paths are configured.
    pub fn resolve(overrides: &RuntimeRootOverrides) -> Result<Self, RuntimeError> {
        let (configuration_roots, configuration_root_source) =
            resolve_configuration_roots(overrides)?;
        let state_root = resolve_state_root(overrides)?;
        let inscriptions_root = resolve_inscriptions_root(overrides, &state_root);
        Ok(Self {
            configuration_roots,
            state_root,
            inscriptions_root,
            configuration_root_source,
        })
    }
}

/// Resolved per-bundle runtime paths. Carries only artifacts that are
/// genuinely per-bundle (tmux socket, runtime directory for inscriptions,
/// startup-failure history, ACP session state). Relay-level artifacts
/// (socket, locks, ready sentinel) live on `RelayRuntimePaths`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleRuntimePaths {
    pub state_root: PathBuf,
    pub bundle_name: String,
    pub runtime_directory: PathBuf,
    pub tmux_socket: PathBuf,
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
            runtime_directory,
        })
    }
}

/// Resolved relay-level runtime paths. The relay binds one socket per
/// instance and holds one runtime/spawn lock; bundle routing is determined
/// by the Hello frame's `bundle_name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRuntimePaths {
    pub state_root: PathBuf,
    pub relay_socket: PathBuf,
    pub relay_lock_file: PathBuf,
    pub relay_spawn_lock_file: PathBuf,
    /// Filesystem sentinel written by the relay host once SIGINT/SIGTERM
    /// handlers are installed and the accept loop has been spawned.
    /// Callers should treat the relay as ready only when both the socket
    /// is connectable AND this sentinel exists.
    pub relay_ready_sentinel: PathBuf,
}

impl RelayRuntimePaths {
    /// Resolves relay-level runtime paths rooted at `state_root`.
    #[must_use]
    pub fn resolve(state_root: &Path) -> Self {
        Self {
            state_root: state_root.to_path_buf(),
            relay_socket: state_root.join(RELAY_SOCKET_FILE),
            relay_lock_file: state_root.join(RELAY_LOCK_FILE),
            relay_spawn_lock_file: state_root.join(RELAY_SPAWN_LOCK_FILE),
            relay_ready_sentinel: state_root.join(RELAY_READY_SENTINEL_FILE),
        }
    }
}

/// Resolves the tmux socket path for one bundle runtime directory.
#[must_use]
pub fn tmux_socket_path_for_runtime_directory(runtime_directory: &Path) -> PathBuf {
    runtime_directory.join(TMUX_SOCKET_FILE)
}

/// Resolves the session pre-shared-key path under the bundle runtime.
///
/// Layout: `<state-root>/bundles/<bundle>/sessions/<session>/identity.psk`.
/// The directory tree is created on first credential provisioning; readers
/// should treat a missing file as the absence of a credential.
#[must_use]
pub fn session_identity_psk_path(
    state_root: &Path,
    bundle_name: &str,
    session_id: &str,
) -> PathBuf {
    state_root
        .join(BUNDLES_DIRECTORY)
        .join(bundle_name)
        .join(SESSIONS_DIRECTORY)
        .join(session_id)
        .join(IDENTITY_PSK_FILE)
}

/// Resolves the peer relay PSK path under the state root.
///
/// Layout: `<state-root>/peers/<peer_alias>.psk`. `peer_alias` is the local
/// portion of the peer's `<id>@RELAY` identifier. Used by the outbound
/// routing slice; the helper is defined here so path conventions remain
/// consistent across slices.
#[must_use]
pub fn peer_relay_psk_path(state_root: &Path, peer_alias: &str) -> PathBuf {
    state_root
        .join(PEERS_DIRECTORY)
        .join(format!("{peer_alias}.psk"))
}

/// Resolves the principal store path at the relay-level state root.
///
/// Layout: `<state-root>/identity/principals.json`. The store is authoritative
/// for credential-to-principal mappings; PSK values are never persisted here.
#[must_use]
pub fn principal_store_path(state_root: &Path) -> PathBuf {
    state_root
        .join(IDENTITY_DIRECTORY)
        .join(PRINCIPAL_STORE_FILE)
}

/// Returns the file mode used for credential artifacts (raw PSKs and the
/// principal store).
#[must_use]
pub fn credential_file_mode() -> u32 {
    CREDENTIAL_FILE_MODE_OWNER_ONLY
}

/// Ensures the bundle runtime directory exists with owner-only permissions.
///
/// # Errors
///
/// Returns a security error when an existing path is owned by another user.
pub fn ensure_bundle_runtime_directory(paths: &BundleRuntimePaths) -> Result<(), RuntimeError> {
    ensure_directory_secure(&paths.runtime_directory)
}

/// Ensures the relay-level runtime directory (`state_root`) exists with
/// owner-only permissions. This is the parent of the relay socket, locks,
/// and ready sentinel.
///
/// # Errors
///
/// Returns a security error when an existing path is owned by another user.
pub fn ensure_relay_runtime_directory(paths: &RelayRuntimePaths) -> Result<(), RuntimeError> {
    ensure_directory_secure(&paths.state_root)
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

/// Resolves the configuration layer list: explicit flags, then environment,
/// then the XDG/home default.
///
/// The first two tiers **replace** the list rather than extending it, so a
/// supplied list is closed and never falls through to a root the operator did
/// not name. The default tier resolves as a single-layer list, so one lookup
/// path serves every tier.
fn resolve_configuration_roots(
    overrides: &RuntimeRootOverrides,
) -> Result<(ConfigurationRoots, ConfigurationRootSource), RuntimeError> {
    if !overrides.configuration_layers.is_empty() {
        let roots = ConfigurationRoots::from_elements(overrides.configuration_layers.clone())
            .map_err(invalid_configuration_layers)?;
        return Ok((roots, ConfigurationRootSource::CommandLine));
    }
    if let Some(value) = env::var(CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE).ok()
        && let Some(roots) = ConfigurationRoots::from_environment_value(&value)
            .map_err(invalid_configuration_layers)?
    {
        return Ok((roots, ConfigurationRootSource::Environment));
    }
    if let Some(path) = env_directory("XDG_CONFIG_HOME") {
        return Ok((
            ConfigurationRoots::single(path.join(APPLICATION_DIRECTORY)),
            ConfigurationRootSource::Default,
        ));
    }
    let home_directory = resolve_home_directory()?;
    Ok((
        ConfigurationRoots::single(configuration_root_from_sources(None, &home_directory)),
        ConfigurationRootSource::Default,
    ))
}

fn invalid_configuration_layers(source: ConfigurationRootsError) -> RuntimeError {
    RuntimeError::validation(
        "validation_invalid_configuration_layers",
        source.to_string(),
    )
}

/// Resolves the state root: the explicit flag, then the environment tier, then
/// XDG, then home. Resolution is identical in every build profile.
///
/// One state root is one relay. Everything that distinguishes two deployments —
/// the relay socket, both locks, the ready sentinel, the principal store, peer
/// credentials — sits at this root rather than beneath a bundle, so isolating a
/// deployment means naming a distinct root and nothing else does it.
fn resolve_state_root(overrides: &RuntimeRootOverrides) -> Result<PathBuf, RuntimeError> {
    if let Some(path) = overrides.state_root.clone() {
        return normalize_state_root(&path);
    }
    if let Some(path) = env_directory(STATE_DIRECTORY_ENVIRONMENT_VARIABLE) {
        return normalize_state_root(&path);
    }
    if let Some(path) = env_directory("XDG_STATE_HOME") {
        return normalize_state_root(&path.join(APPLICATION_DIRECTORY));
    }
    let home_directory = resolve_home_directory()?;
    normalize_state_root(&state_root_from_sources(None, &home_directory))
}

/// Normalizes a resolved state root to a non-empty absolute path.
///
/// This is a precondition for propagation rather than tidiness. The root is
/// stamped into every spawned member's environment, and a relative value
/// re-resolves against each child's working directory — members routinely
/// declare their own — so the child would silently address a different state
/// root than the relay that spawned it, find no socket there, and report the
/// relay unavailable.
///
/// Empty is rejected rather than normalized. The environment tier reads blank as
/// absent, so accepting an empty flag would give one spelling of "nothing" two
/// meanings depending on which surface carried it.
fn normalize_state_root(path: &Path) -> Result<PathBuf, RuntimeError> {
    if path.as_os_str().is_empty() {
        return Err(RuntimeError::validation(
            "validation_invalid_state_directory",
            "state directory must not be empty".to_string(),
        ));
    }
    std::path::absolute(path).map_err(|source| {
        RuntimeError::io(
            format!("resolve state directory {}", path.display()),
            source,
        )
    })
}

/// Resolves the inscriptions root, which defaults beneath the state root and so
/// follows it without separate selection.
fn resolve_inscriptions_root(overrides: &RuntimeRootOverrides, state_root: &Path) -> PathBuf {
    overrides
        .inscriptions_root
        .clone()
        .unwrap_or_else(|| state_root.join(INSCRIPTIONS_DIRECTORY_DEFAULT))
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

/// The canonical bundle-name grammar: non-empty and limited to ASCII
/// alphanumeric, `-`, `_`, or `.`. This is the single source of truth for what
/// a bundle namespace may contain; callers that also join the name into a path
/// segment must additionally reject the traversal-only `.` / `..` segments.
pub(crate) fn is_valid_bundle_name(bundle_name: &str) -> bool {
    !bundle_name.is_empty()
        && bundle_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn validate_bundle_name(bundle_name: &str) -> Result<(), RuntimeError> {
    if is_valid_bundle_name(bundle_name) {
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
