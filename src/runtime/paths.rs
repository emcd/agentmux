//! Runtime path resolution for configuration, state, and bundle sockets.

use std::{
    env, fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use super::error::RuntimeError;
use super::inscriptions::emit_inscription;

const APPLICATION_DIRECTORY: &str = "agentmux";
/// Environment tier of configuration-root resolution. Ranked below the CLI flag
/// and above discovery, and like the flag it replaces the root outright.
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
    pub configuration_root: Option<PathBuf>,
    pub state_root: Option<PathBuf>,
    pub inscriptions_root: Option<PathBuf>,
    /// Feeds state and inscriptions root resolution only. Configuration root
    /// resolution ignores it.
    pub repository_root: Option<PathBuf>,
    /// Enables nearest-ancestor discovery of a configuration root. Off unless
    /// requested, so a repository-local root is never silently preferred over
    /// the user-level one.
    pub discover_local_configuration: bool,
}

/// Tier which supplied the configuration root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigurationRootSource {
    /// `--configuration-directory`.
    CommandLine,
    /// `AGENTMUX_CONFIGURATION_DIRECTORY`.
    Environment,
    /// Nearest-ancestor discovery.
    Discovered,
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
    pub configuration_root: PathBuf,
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
        let (configuration_root, configuration_root_source) =
            resolve_configuration_root(overrides)?;
        let state_root = resolve_state_root(overrides)?;
        let inscriptions_root = resolve_inscriptions_root(overrides, &state_root);
        Ok(Self {
            configuration_root,
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

/// Resolves the debug repository-local state root.
pub fn debug_repository_state_root(repository_root: &Path) -> PathBuf {
    repository_root
        .join(".auxiliary/state")
        .join(APPLICATION_DIRECTORY)
}

/// Resolves the configuration root a directory would supply if it hosts one.
///
/// This is the shape ancestor discovery looks for. It describes only where a
/// tree keeps Agentmux configuration, so a tree which is not an Agentmux
/// checkout can legitimately host configuration; nothing here inspects build
/// profile, Git metadata, or package manifests.
pub fn local_configuration_root(directory: &Path) -> PathBuf {
    directory
        .join(".auxiliary/configuration")
        .join(APPLICATION_DIRECTORY)
}

/// Resolves the debug repository-local inscriptions root.
pub fn debug_repository_inscriptions_root(repository_root: &Path) -> PathBuf {
    repository_root
        .join(".auxiliary/inscriptions")
        .join(APPLICATION_DIRECTORY)
}

/// Resolves the Agentmux source checkout supplying repository-local (dev-mode)
/// state and inscriptions, or `None` when the working directory sits in none.
///
/// This is the sole repository-root resolver. Every surface — CLI, TUI, MCP
/// host, relay host — resolves through it, because a surface that answered
/// differently would look for the relay socket somewhere the relay never bound
/// it, and the two would never meet.
///
/// Git supplies the candidate and the package manifest confirms it, because
/// each covers what the other cannot:
///
/// Git makes the answer stable across a project's linked worktrees. The common
/// directory is shared by a primary clone and every worktree linked to it, so
/// its owner root is one path no matter which worktree the process runs in, and
/// every principal in the project therefore agrees on one state root and one
/// relay socket. Probing the working directory alone answers with the worktree,
/// which would give each sibling worktree a private relay and sever the
/// cross-worktree coordination Agentmux exists to provide. Git also searches
/// ancestors, so a process launched anywhere beneath a checkout resolves it.
///
/// The manifest marker keeps the answer honest. Git alone adopts whichever
/// repository the process happens to stand in, so an unrelated clone would
/// become an Agentmux state root; requiring the owner root to declare
/// `name = "agentmux"` confines dev-mode to an actual Agentmux checkout. When
/// Git resolves a repository whose owner root fails that check, the reason is
/// reported on stderr so an operator can see why production paths were
/// selected. Stderr is the only channel available: this runs during root
/// resolution, before any inscriptions sink is configured.
///
/// Worktrees owned by a bare repository are not supported. Their common
/// directory is conventionally `<name>.git` rather than an ancestor named
/// `.git`, and a bare repository has no checked-out root to carry the manifest,
/// so both halves of the answer are unavailable. Such a deployment resolves
/// production paths.
///
/// Always `None` in release builds, where dev-mode roots are unreachable and
/// both the subprocess and the manifest read would be wasted work.
///
/// This is the last Git dependency in Agentmux, and it exists only to feed the
/// build-profile-gated repository-local branches of [`resolve_state_root`] and
/// [`resolve_inscriptions_root`]. Deleting it without deleting those branches
/// leaves the repository root permanently unresolved and silently collapses
/// repository-local state onto the XDG default. The runtime-instance work that
/// replaces the provenance removes the branches, and this resolver goes with
/// them.
#[must_use]
pub fn repository_checkout_root(current_directory: &Path) -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let common_directory = git_common_directory(current_directory)?;
    let repository_root = repository_root_from_git_common_directory(&common_directory)?;
    if !cargo_manifest_declares_agentmux(&repository_root) {
        eprintln!(
            "debug build launched inside a Git repository that is not an Agentmux source \
             checkout ({}); using production runtime paths",
            repository_root.display()
        );
        return None;
    }
    Some(repository_root)
}

/// Asks Git for the common directory — the one a primary clone shares with
/// every worktree linked to it.
fn git_common_directory(current_directory: &Path) -> Option<PathBuf> {
    let common_directory = run_git(
        current_directory,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    // `--path-format` is absent from older Git; the bare form answers relative
    // to the working directory, so the result is joined onto it below.
    .or_else(|| run_git(current_directory, &["rev-parse", "--git-common-dir"]))
    .map(PathBuf::from)?;
    if common_directory.is_absolute() {
        return Some(common_directory);
    }
    Some(current_directory.join(common_directory))
}

/// Walks a Git common directory up to the repository root owning it: the parent
/// of `.git`. A linked worktree's common directory points into the *owner's*
/// `.git`, which is what makes every worktree of a project resolve one root.
fn repository_root_from_git_common_directory(common_directory: &Path) -> Option<PathBuf> {
    let mut cursor = Some(common_directory);
    while let Some(path) = cursor {
        if path.file_name().is_some_and(|name| name == ".git") {
            return path.parent().map(Path::to_path_buf);
        }
        cursor = path.parent();
    }
    None
}

/// Runs Git in `directory`, yielding trimmed stdout on success.
///
/// The ambient Git environment is cleared so the answer describes the directory
/// asked about rather than whichever repository invoked this process — an agent
/// spawned from a Git hook or a worktree command otherwise inherits a `GIT_DIR`
/// pointing somewhere else entirely.
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
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return None;
    }
    Some(value)
}

/// Reports whether the `Cargo.toml` at `root` declares `name = "agentmux"`
/// in its `[package]` table. Unreadable or unparseable manifests are not
/// checkouts.
fn cargo_manifest_declares_agentmux(root: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let Ok(manifest) = raw.parse::<toml::Value>() else {
        return false;
    };
    manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        == Some(APPLICATION_DIRECTORY)
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

/// Resolves the configuration root: explicit flag, then environment, then
/// opt-in ancestor discovery, then the XDG/home default.
///
/// The first two tiers **replace** the root rather than extending a search
/// list, so an explicitly supplied root never falls through to a different one
/// for a file it does not define. Resolution is identical in every build
/// profile: unlike the state and inscriptions roots below, nothing here needs
/// to keep a source-tree deployment from colliding with an installed one.
fn resolve_configuration_root(
    overrides: &RuntimeRootOverrides,
) -> Result<(PathBuf, ConfigurationRootSource), RuntimeError> {
    if let Some(path) = overrides.configuration_root.clone() {
        return Ok((path, ConfigurationRootSource::CommandLine));
    }
    if let Some(path) = env_directory(CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE) {
        return Ok((path, ConfigurationRootSource::Environment));
    }
    if overrides.discover_local_configuration
        && let Some(path) = discover_configuration_root()
    {
        return Ok((path, ConfigurationRootSource::Discovered));
    }
    if let Some(path) = env_directory("XDG_CONFIG_HOME") {
        return Ok((
            path.join(APPLICATION_DIRECTORY),
            ConfigurationRootSource::Default,
        ));
    }
    let home_directory = resolve_home_directory()?;
    Ok((
        configuration_root_from_sources(None, &home_directory),
        ConfigurationRootSource::Default,
    ))
}

/// Walks the working directory and its ancestors for a directory hosting an
/// Agentmux configuration root, nearest ancestor winning.
///
/// Enumeration starts at the canonicalized working directory so symbolic links
/// resolve consistently, and terminates at the filesystem root. The selection
/// is reported on stderr rather than stdout: `host mcp` serves the protocol
/// over stdout, where a diagnostic would corrupt the stream.
fn discover_configuration_root() -> Option<PathBuf> {
    let working_directory = env::current_dir().ok()?;
    let working_directory = working_directory
        .canonicalize()
        .unwrap_or(working_directory);
    for ancestor in working_directory.ancestors() {
        let candidate = local_configuration_root(ancestor);
        if !candidate.is_dir() {
            continue;
        }
        let candidate = candidate.canonicalize().unwrap_or(candidate);
        emit_inscription(
            "runtime.configuration.discovered_root",
            &serde_json::json!({
                "working_directory": working_directory,
                "configuration_root": candidate,
            }),
        );
        eprintln!(
            "discovered local configuration root: {}",
            candidate.display()
        );
        return Some(candidate);
    }
    None
}

/// Resolves the state root.
///
/// The repository-local branch stays gated on build profile deliberately. It is
/// currently the only thing keeping a source-tree relay and an installed relay
/// from resolving the same socket, locks, ready sentinel, principal store, and
/// peer credentials, all of which are relay-wide rather than per-bundle.
/// Ungating it without runtime instances would collapse two live deployments
/// into one, stranding sessions on one relay while new clients attach to
/// another. Runtime instances replace this mechanism; until then it stays, and
/// so does the Git-derived repository-root provenance which activates it.
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

/// Resolves the inscriptions root. Gated on build profile for the same reason
/// as [`resolve_state_root`], and removed by the same deferred work.
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
