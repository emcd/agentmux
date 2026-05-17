//! Bundle configuration loading and sender-association helpers.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::{Display, Formatter},
    fs, io,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::{Deserialize, Serialize};

const BUNDLE_SCHEMA_VERSION: u32 = 1;
const POLICIES_SCHEMA_VERSION: u32 = 1;
const CODERS_FILE: &str = "coders.toml";
const BUNDLES_DIRECTORY: &str = "bundles";
const BUNDLE_EXTENSION: &str = "toml";
const USERS_FILE: &str = "users.toml";
const POLICIES_FILE: &str = "policies.toml";
const SESSION_ID_LENGTH_MAX: usize = 31;
const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";
pub const RESERVED_GROUP_ALL: &str = "ALL";

/// Declared session type for one configured session entry.
///
/// The session type is fixed at operator configuration time by the single
/// session-type subtable on a `[[sessions]]` entry. It is never asserted by a
/// client at connect time.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    Tmux,
    Acp,
    Ui,
    Pubsub,
}

/// One configured bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleMember {
    /// Canonical routing identity from `[[sessions]].id`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional human-facing recipient label from `[[sessions]].name`.
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    /// Declared session type and its delivery target configuration.
    pub target: TargetConfiguration,
    /// Optional persistent agent session handle sourced from the active
    /// session-type subtable's `coder-session-id` (not from `[[coders]]`).
    /// ACP delivery uses this to select `session/load` vs `session/new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coder_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
}

/// Optional prompt-readiness template for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PromptReadinessTemplate {
    pub prompt_regex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_idle_cursor_column: Option<usize>,
}

/// Validated session type and delivery target configuration for one member.
///
/// `Tmux` and `Acp` carry transport configuration; `Ui` and `Pubsub` are bare
/// markers whose delivery paths are not yet implemented.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "transport", content = "config")]
pub enum TargetConfiguration {
    Tmux(TmuxTargetConfiguration),
    Acp(AcpTargetConfiguration),
    Ui,
    Pubsub,
}

impl TargetConfiguration {
    /// Reports the declared session type for this target.
    #[must_use]
    pub fn session_type(&self) -> SessionType {
        match self {
            Self::Tmux(_) => SessionType::Tmux,
            Self::Acp(_) => SessionType::Acp,
            Self::Ui => SessionType::Ui,
            Self::Pubsub => SessionType::Pubsub,
        }
    }
}

/// Tmux transport configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TmuxTargetConfiguration {
    pub start_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_readiness: Option<PromptReadinessTemplate>,
}

/// ACP transport configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AcpTargetConfiguration {
    pub channel: AcpChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<NameValueEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<NameValueEntry>,
}

/// Configuration for one named bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConfiguration {
    pub schema_version: String,
    pub bundle_name: String,
    pub autostart: bool,
    pub groups: Vec<String>,
    pub members: Vec<BundleMember>,
}

/// Group membership metadata for one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleGroupMembership {
    pub bundle_name: String,
    pub autostart: bool,
    pub groups: Vec<String>,
}

/// One global user session entry from `users.toml`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TuiSession {
    /// Canonical global identity in `session@GLOBAL` form.
    pub id: String,
    /// Optional operator-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Policy preset reference.
    pub policy: String,
    /// Declared session type for this global user entry.
    pub session_type: SessionType,
}

/// Global user configuration loaded from `users.toml`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TuiConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_session: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<TuiSession>,
}

impl TuiConfiguration {
    #[must_use]
    pub fn session_by_id(&self, selector: &str) -> Option<&TuiSession> {
        self.sessions.iter().find(|session| session.id == selector)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawCodersFile {
    format_version: u32,
    #[serde(default)]
    coders: Vec<RawCoder>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawCoder {
    id: String,
    #[serde(default)]
    tmux: Option<RawTmuxTarget>,
    #[serde(default)]
    acp: Option<RawAcpTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawTmuxTarget {
    initial_command: String,
    resume_command: String,
    #[serde(default)]
    prompt_regex: Option<String>,
    #[serde(default)]
    prompt_inspect_lines: Option<usize>,
    #[serde(default)]
    prompt_idle_column: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawAcpTarget {
    channel: AcpChannel,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    turn_timeout_ms: Option<u64>,
    #[serde(default)]
    headers: Vec<NameValueEntry>,
    #[serde(default)]
    environment: Vec<NameValueEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NameValueEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AcpChannel {
    Stdio,
    Http,
}

#[derive(Clone, Debug)]
struct Coder {
    target: CoderTarget,
}

#[derive(Clone, Debug)]
enum CoderTarget {
    Tmux(TmuxTarget),
    Acp(AcpTarget),
}

#[derive(Clone, Debug)]
struct TmuxTarget {
    initial_command: String,
    resume_command: String,
    prompt_regex: Option<String>,
    prompt_inspect_lines: Option<usize>,
    prompt_idle_column: Option<usize>,
}

#[derive(Clone, Debug)]
struct AcpTarget {
    channel: AcpChannel,
    command: Option<String>,
    url: Option<String>,
    turn_timeout_ms: Option<u64>,
    headers: Vec<NameValueEntry>,
    environment: Vec<NameValueEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawBundleFile {
    format_version: u32,
    #[serde(default)]
    autostart: bool,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    sessions: Vec<RawSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawUsersFile {
    #[serde(default)]
    default_bundle: Option<String>,
    #[serde(default)]
    default_session: Option<String>,
    #[serde(default)]
    sessions: Vec<RawUsersSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawUsersSession {
    id: String,
    #[serde(default)]
    name: Option<String>,
    policy: String,
    #[serde(default)]
    ui: Option<RawSessionMarker>,
    #[serde(default)]
    pubsub: Option<RawSessionMarker>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPoliciesFile {
    format_version: u32,
    #[serde(default, rename = "default")]
    _default: Option<String>,
    #[serde(default)]
    policies: Vec<RawPolicyPreset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPolicyPreset {
    id: String,
    #[serde(default, rename = "description")]
    _description: Option<String>,
    #[serde(default, rename = "controls")]
    _controls: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawSession {
    id: String,
    #[serde(default)]
    name: Option<String>,
    directory: PathBuf,
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    coder: Option<String>,
    #[serde(default)]
    coder_session_id: Option<String>,
    #[serde(default)]
    ui: Option<RawSessionMarker>,
    #[serde(default)]
    pubsub: Option<RawSessionMarker>,
}

/// A `[sessions.ui]` or `[sessions.pubsub]` subtable; an empty body is valid.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessionMarker {}

/// Configuration load/validation failures.
#[derive(Debug)]
pub enum ConfigurationError {
    UnknownBundle {
        bundle_name: String,
        path: PathBuf,
    },
    AmbiguousSender {
        working_directory: PathBuf,
        matches: Vec<String>,
    },
    InvalidConfiguration {
        path: PathBuf,
        message: String,
    },
    InvalidGroupName {
        path: PathBuf,
        group_name: String,
    },
    ReservedGroupName {
        path: PathBuf,
        group_name: String,
    },
    Io {
        context: String,
        source: io::Error,
    },
}

impl ConfigurationError {
    fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self::InvalidConfiguration {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

impl Display for ConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBundle { bundle_name, path } => write!(
                formatter,
                "bundle '{}' is not configured at {}",
                bundle_name,
                path.display()
            ),
            Self::AmbiguousSender {
                working_directory,
                matches,
            } => write!(
                formatter,
                "ambiguous sender for {} matched sessions: {}",
                working_directory.display(),
                matches.join(", ")
            ),
            Self::InvalidConfiguration { path, message } => {
                write!(
                    formatter,
                    "invalid bundle configuration {}: {}",
                    path.display(),
                    message
                )
            }
            Self::InvalidGroupName { path, group_name } => write!(
                formatter,
                "invalid group name '{}' in {}",
                group_name,
                path.display()
            ),
            Self::ReservedGroupName { path, group_name } => write!(
                formatter,
                "group name '{}' is reserved in {}",
                group_name,
                path.display()
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Resolves path to shared coder definitions.
pub fn coders_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(CODERS_FILE)
}

/// Resolves path to one bundle definition file.
pub fn bundle_configuration_path(configuration_root: &Path, bundle_name: &str) -> PathBuf {
    configuration_root
        .join(BUNDLES_DIRECTORY)
        .join(format!("{bundle_name}.{BUNDLE_EXTENSION}"))
}

/// Resolves path to global user configuration file (`users.toml`).
pub fn tui_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(USERS_FILE)
}

/// Resolves path to authorization policy presets file.
pub fn policies_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(POLICIES_FILE)
}

/// Loads bundle-group membership metadata for configured bundles.
///
/// # Errors
///
/// Returns `ConfigurationError` for malformed bundle files and I/O failures.
pub fn load_bundle_group_memberships(
    configuration_root: &Path,
) -> Result<Vec<BundleGroupMembership>, ConfigurationError> {
    let bundles_directory = configuration_root.join(BUNDLES_DIRECTORY);
    if !bundles_directory.exists() {
        return Ok(Vec::new());
    }
    let mut bundle_names = fs::read_dir(&bundles_directory)
        .map_err(|source| {
            ConfigurationError::io(
                format!("read bundle directory {}", bundles_directory.display()),
                source,
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.path().file_name().map(ToOwned::to_owned))
        .filter_map(|name| name.to_str().map(ToOwned::to_owned))
        .filter(|name| name.ends_with(".toml"))
        .filter_map(|name| name.strip_suffix(".toml").map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    bundle_names.sort_unstable();

    let mut memberships = Vec::with_capacity(bundle_names.len());
    for bundle_name in bundle_names {
        let bundle_path = bundle_configuration_path(configuration_root, &bundle_name);
        let bundle_raw = fs::read_to_string(&bundle_path).map_err(|source| {
            ConfigurationError::io(format!("read {}", bundle_path.display()), source)
        })?;
        let bundle_file = toml::from_str::<RawBundleFile>(&bundle_raw).map_err(|source| {
            ConfigurationError::InvalidConfiguration {
                path: bundle_path.clone(),
                message: source.to_string(),
            }
        })?;
        validate_format_version(
            bundle_file.format_version,
            BUNDLE_SCHEMA_VERSION,
            &bundle_path,
        )?;
        if bundle_file.sessions.is_empty() {
            continue;
        }
        let groups = validate_bundle_groups(&bundle_file.groups, &bundle_path)?;
        memberships.push(BundleGroupMembership {
            bundle_name,
            autostart: bundle_file.autostart,
            groups,
        });
    }
    Ok(memberships)
}

/// Loads one bundle configuration and applies schema validation.
///
/// # Errors
///
/// Returns `ConfigurationError` for unknown bundles, invalid schema, and I/O.
pub fn load_bundle_configuration(
    configuration_root: &Path,
    bundle_name: &str,
) -> Result<BundleConfiguration, ConfigurationError> {
    let coders_path = coders_configuration_path(configuration_root);
    let bundle_path = bundle_configuration_path(configuration_root, bundle_name);

    if !bundle_path.exists() {
        return Err(ConfigurationError::UnknownBundle {
            bundle_name: bundle_name.to_string(),
            path: bundle_path,
        });
    }

    let coders_raw = fs::read_to_string(&coders_path).map_err(|source| {
        ConfigurationError::io(format!("read {}", coders_path.display()), source)
    })?;
    let bundle_raw = fs::read_to_string(&bundle_path).map_err(|source| {
        ConfigurationError::io(format!("read {}", bundle_path.display()), source)
    })?;

    let coders_file = toml::from_str::<RawCodersFile>(&coders_raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: coders_path.clone(),
            message: source.to_string(),
        }
    })?;
    let bundle_file = toml::from_str::<RawBundleFile>(&bundle_raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: bundle_path.clone(),
            message: source.to_string(),
        }
    })?;

    validate_loaded_configuration(
        bundle_name,
        coders_file,
        &coders_path,
        bundle_file,
        &bundle_path,
    )
}

/// Loads global user configuration from `<config-root>/users.toml`.
///
/// # Errors
///
/// Returns `ConfigurationError` when the file exists but is malformed.
pub fn load_tui_configuration(
    configuration_root: &Path,
) -> Result<Option<TuiConfiguration>, ConfigurationError> {
    load_tui_configuration_file(&tui_configuration_path(configuration_root))
}

/// Loads global user configuration from an explicit file path.
///
/// # Errors
///
/// Returns `ConfigurationError` when the file exists but is malformed.
pub fn load_tui_configuration_file(
    path: &Path,
) -> Result<Option<TuiConfiguration>, ConfigurationError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = toml::from_str::<RawUsersFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;

    let default_bundle = parsed
        .default_bundle
        .as_deref()
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let default_session = parsed
        .default_session
        .as_deref()
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let sessions = validate_tui_sessions(parsed.sessions, path)?;

    Ok(Some(TuiConfiguration {
        default_bundle,
        default_session,
        sessions,
    }))
}

/// Loads known policy preset identifiers from `<config-root>/policies.toml`.
///
/// # Errors
///
/// Returns `ConfigurationError` when the artifact is missing or malformed.
pub fn load_policy_ids(configuration_root: &Path) -> Result<HashSet<String>, ConfigurationError> {
    let path = policies_configuration_path(configuration_root);
    let raw = fs::read_to_string(&path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = toml::from_str::<RawPoliciesFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.clone(),
            message: source.to_string(),
        }
    })?;
    validate_format_version(parsed.format_version, POLICIES_SCHEMA_VERSION, &path)?;

    let mut unique = HashSet::<String>::new();
    for policy in parsed.policies {
        let policy_id = normalize_field(policy.id.as_str());
        if policy_id.is_empty() {
            return Err(ConfigurationError::invalid(
                &path,
                "policy id must be non-empty",
            ));
        }
        if !unique.insert(policy_id.to_string()) {
            return Err(ConfigurationError::invalid(
                &path,
                format!("duplicate policy id '{policy_id}'"),
            ));
        }
    }
    Ok(unique)
}

/// Infers sender session from bundle member working-directory matches.
///
/// # Errors
///
/// Returns `ConfigurationError::AmbiguousSender` when more than one member
/// matches the same directory.
pub fn infer_sender_from_working_directory(
    bundle: &BundleConfiguration,
    working_directory: &Path,
) -> Result<Option<String>, ConfigurationError> {
    let target = canonicalize_best_effort(working_directory);
    let mut matches = Vec::new();

    for member in &bundle.members {
        let Some(member_directory) = member.working_directory.as_ref() else {
            continue;
        };
        if canonicalize_best_effort(member_directory) == target {
            matches.push(member.id.clone());
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(ConfigurationError::AmbiguousSender {
            working_directory: target,
            matches,
        }),
    }
}

fn validate_loaded_configuration(
    expected_bundle_name: &str,
    coders_file: RawCodersFile,
    coders_path: &Path,
    bundle_file: RawBundleFile,
    bundle_path: &Path,
) -> Result<BundleConfiguration, ConfigurationError> {
    validate_format_version(
        coders_file.format_version,
        BUNDLE_SCHEMA_VERSION,
        coders_path,
    )?;
    validate_format_version(
        bundle_file.format_version,
        BUNDLE_SCHEMA_VERSION,
        bundle_path,
    )?;

    let coders = validate_coders(coders_file.coders, coders_path)?;

    let groups = validate_bundle_groups(&bundle_file.groups, bundle_path)?;

    if bundle_file.sessions.is_empty() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            "sessions must contain at least one session",
        ));
    }

    let mut session_ids = HashSet::new();
    let mut session_names = HashSet::new();
    let mut members = Vec::with_capacity(bundle_file.sessions.len());

    for session in &bundle_file.sessions {
        let session_id = normalize_field(session.id.as_str());
        if session_id.is_empty() {
            return Err(ConfigurationError::invalid(
                bundle_path,
                "session id must be non-empty",
            ));
        }
        validate_session_id(bundle_path, session_id)?;
        if !session_ids.insert(session_id.to_string()) {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("duplicate session id '{session_id}'"),
            ));
        }

        let session_name = session
            .name
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty());
        if let Some(session_name) = session_name
            && !session_names.insert(session_name.to_string())
        {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("duplicate session name '{session_name}'"),
            ));
        }

        if session.directory.as_os_str().is_empty() {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("session '{session_id}' directory must be non-empty"),
            ));
        }

        let policy_id = session
            .policy
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        let (target, coder_session_id) =
            build_session_target(session, &coders, coders_path, bundle_path, session_id)?;

        members.push(BundleMember {
            id: session_id.to_string(),
            name: session_name.map(ToString::to_string),
            working_directory: Some(session.directory.clone()),
            target,
            coder_session_id,
            policy_id,
        });
    }

    Ok(BundleConfiguration {
        schema_version: BUNDLE_SCHEMA_VERSION.to_string(),
        bundle_name: expected_bundle_name.to_string(),
        autostart: bundle_file.autostart,
        groups,
        members,
    })
}

/// Resolves a bundle member's validated delivery target.
///
/// A session is coder-backed when it carries a `coder` reference; its transport
/// (tmux or ACP) is derived from that coder's descriptor. Coder-less sessions
/// declare exactly one of the `[sessions.ui]` or `[sessions.pubsub]` markers.
fn build_session_target(
    session: &RawSession,
    coders: &HashMap<String, Coder>,
    coders_path: &Path,
    bundle_path: &Path,
    session_id: &str,
) -> Result<(TargetConfiguration, Option<String>), ConfigurationError> {
    let kind = select_session_kind(
        session.coder.is_some(),
        session.ui.is_some(),
        session.pubsub.is_some(),
        bundle_path,
        session_id,
    )?;
    match kind {
        SessionKind::Coder => {
            let coder_session_id = normalize_optional(session.coder_session_id.as_deref());
            let coder = resolve_session_coder(
                session.coder.as_deref().unwrap_or_default(),
                coders,
                bundle_path,
                session_id,
            )?;
            match &coder.target {
                CoderTarget::Tmux(tmux_target) => {
                    let command_template = if coder_session_id.is_some() {
                        tmux_target.resume_command.as_str()
                    } else {
                        tmux_target.initial_command.as_str()
                    };
                    let start_command = render_command_template(
                        command_template,
                        coder_session_id.as_deref(),
                        bundle_path,
                        session_id,
                    )?;
                    let prompt_readiness =
                        prompt_readiness_from_tmux_target(tmux_target, coders_path, session_id)?;
                    Ok((
                        TargetConfiguration::Tmux(TmuxTargetConfiguration {
                            start_command,
                            prompt_readiness,
                        }),
                        coder_session_id,
                    ))
                }
                CoderTarget::Acp(acp_target) => Ok((
                    TargetConfiguration::Acp(AcpTargetConfiguration {
                        channel: acp_target.channel,
                        command: acp_target.command.clone(),
                        url: acp_target.url.clone(),
                        turn_timeout_ms: acp_target.turn_timeout_ms,
                        headers: acp_target.headers.clone(),
                        environment: acp_target.environment.clone(),
                    }),
                    coder_session_id,
                )),
            }
        }
        SessionKind::Ui => {
            reject_coder_session_id(session, bundle_path, session_id)?;
            Ok((TargetConfiguration::Ui, None))
        }
        SessionKind::Pubsub => {
            reject_coder_session_id(session, bundle_path, session_id)?;
            Ok((TargetConfiguration::Pubsub, None))
        }
    }
}

/// Rejects a `coder-session-id` declared on a coder-less session entry.
fn reject_coder_session_id(
    session: &RawSession,
    bundle_path: &Path,
    session_id: &str,
) -> Result<(), ConfigurationError> {
    if session.coder_session_id.is_some() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            format!("coder-less session '{session_id}' must not declare coder-session-id"),
        ));
    }
    Ok(())
}

fn resolve_session_coder<'a>(
    coder: &str,
    coders: &'a HashMap<String, Coder>,
    bundle_path: &Path,
    session_id: &str,
) -> Result<&'a Coder, ConfigurationError> {
    let coder_id = normalize_field(coder);
    if coder_id.is_empty() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            format!("session '{session_id}' coder reference must be non-empty"),
        ));
    }
    coders.get(coder_id).ok_or_else(|| {
        ConfigurationError::invalid(
            bundle_path,
            format!("session '{session_id}' references unknown coder '{coder_id}'"),
        )
    })
}

/// The declared shape of a bundle session: coder-backed, or one of the
/// coder-less marker types.
#[derive(Clone, Copy)]
enum SessionKind {
    Coder,
    Ui,
    Pubsub,
}

/// Selects the single declared session kind, rejecting a session that declares
/// neither a coder reference nor a coder-less marker, or more than one.
fn select_session_kind(
    has_coder: bool,
    has_ui: bool,
    has_pubsub: bool,
    path: &Path,
    session_id: &str,
) -> Result<SessionKind, ConfigurationError> {
    let declared = [
        (has_coder, SessionKind::Coder),
        (has_ui, SessionKind::Ui),
        (has_pubsub, SessionKind::Pubsub),
    ];
    let present: Vec<SessionKind> = declared
        .into_iter()
        .filter_map(|(declared, kind)| declared.then_some(kind))
        .collect();
    match present.as_slice() {
        [kind] => Ok(*kind),
        [] => Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' must declare a coder reference or exactly one \
                 coder-less session-type subtable ([sessions.ui] or [sessions.pubsub])"
            ),
        )),
        _ => Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' declares multiple session types; expected a coder \
                 reference or exactly one of [sessions.ui] or [sessions.pubsub]"
            ),
        )),
    }
}

/// Selects the session type for a global user, which is always coder-less.
fn select_marker_session_type(
    has_ui: bool,
    has_pubsub: bool,
    path: &Path,
    session_id: &str,
) -> Result<SessionType, ConfigurationError> {
    match (has_ui, has_pubsub) {
        (true, false) => Ok(SessionType::Ui),
        (false, true) => Ok(SessionType::Pubsub),
        (false, false) => Err(ConfigurationError::invalid(
            path,
            format!(
                "users session '{session_id}' must declare exactly one session-type \
                 subtable ([sessions.ui] or [sessions.pubsub])"
            ),
        )),
        (true, true) => Err(ConfigurationError::invalid(
            path,
            format!(
                "users session '{session_id}' declares multiple session-type subtables; \
                 expected exactly one"
            ),
        )),
    }
}

fn validate_tui_sessions(
    sessions: Vec<RawUsersSession>,
    path: &Path,
) -> Result<Vec<TuiSession>, ConfigurationError> {
    let mut unique = HashSet::<String>::new();
    let mut validated = Vec::<TuiSession>::with_capacity(sessions.len());
    for session in sessions {
        let selector_id = normalize_field(session.id.as_str());
        if selector_id.is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                "users session id must be non-empty",
            ));
        }
        validate_global_session_id(path, selector_id)?;
        if !unique.insert(selector_id.to_string()) {
            return Err(ConfigurationError::invalid(
                path,
                format!("duplicate users session id '{selector_id}'"),
            ));
        }

        let policy_id = normalize_field(session.policy.as_str());
        if policy_id.is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("users session '{selector_id}' policy must be non-empty"),
            ));
        }
        let session_type = select_marker_session_type(
            session.ui.is_some(),
            session.pubsub.is_some(),
            path,
            selector_id,
        )?;
        let name = session
            .name
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        validated.push(TuiSession {
            id: selector_id.to_string(),
            name,
            policy: policy_id.to_string(),
            session_type,
        });
    }
    Ok(validated)
}

fn validate_coders(
    coders: Vec<RawCoder>,
    coders_path: &Path,
) -> Result<HashMap<String, Coder>, ConfigurationError> {
    if coders.is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            "coders must contain at least one coder",
        ));
    }

    let mut unique = HashMap::new();
    for coder in coders {
        let coder_id = normalize_field(coder.id.as_str());
        if coder_id.is_empty() {
            return Err(ConfigurationError::invalid(
                coders_path,
                "coder id must be non-empty",
            ));
        }
        if unique.contains_key(coder_id) {
            return Err(ConfigurationError::invalid(
                coders_path,
                format!("duplicate coder id '{coder_id}'"),
            ));
        }

        let target = match (coder.tmux, coder.acp) {
            (Some(tmux), None) => {
                CoderTarget::Tmux(validate_tmux_target(tmux, coders_path, coder_id)?)
            }
            (None, Some(acp)) => CoderTarget::Acp(validate_acp_target(acp, coders_path, coder_id)?),
            (None, None) => {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!(
                        "coder '{coder_id}' must define exactly one target table ([coders.tmux] or [coders.acp])"
                    ),
                ));
            }
            (Some(_), Some(_)) => {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!(
                        "coder '{coder_id}' defines multiple target tables; expected exactly one"
                    ),
                ));
            }
        };

        unique.insert(coder_id.to_string(), Coder { target });
    }

    Ok(unique)
}

fn validate_bundle_groups(
    groups: &[String],
    bundle_path: &Path,
) -> Result<Vec<String>, ConfigurationError> {
    let mut validated = Vec::<String>::with_capacity(groups.len());
    let mut seen = HashSet::<String>::new();
    for raw_group in groups {
        let group = normalize_field(raw_group.as_str());
        if group.is_empty() {
            return Err(ConfigurationError::InvalidGroupName {
                path: bundle_path.to_path_buf(),
                group_name: raw_group.clone(),
            });
        }
        if group == RESERVED_GROUP_ALL {
            return Err(ConfigurationError::ReservedGroupName {
                path: bundle_path.to_path_buf(),
                group_name: group.to_string(),
            });
        }
        if is_reserved_group_name(group) || !is_custom_group_name(group) {
            return Err(ConfigurationError::InvalidGroupName {
                path: bundle_path.to_path_buf(),
                group_name: group.to_string(),
            });
        }
        if seen.insert(group.to_string()) {
            validated.push(group.to_string());
        }
    }
    Ok(validated)
}

fn is_reserved_group_name(group: &str) -> bool {
    group.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
    })
}

fn is_custom_group_name(group: &str) -> bool {
    group.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    })
}

fn validate_format_version(
    version: u32,
    expected: u32,
    path: &Path,
) -> Result<(), ConfigurationError> {
    if version == expected {
        return Ok(());
    }
    Err(ConfigurationError::invalid(
        path,
        format!("unsupported format-version '{version}'; expected '{expected}'"),
    ))
}

fn render_command_template(
    template: &str,
    coder_session_id: Option<&str>,
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let mut rendered = template.to_string();

    if rendered.contains("{coder-session-id}") {
        let Some(coder_session_id) = coder_session_id else {
            return Err(ConfigurationError::invalid(
                path,
                format!("session '{session_id}' requires coder-session-id for template"),
            ));
        };
        rendered = rendered.replace("{coder-session-id}", coder_session_id);
    }

    let placeholder_regex = Regex::new(r"\{[a-z][a-z0-9-]*\}").map_err(|source| {
        ConfigurationError::invalid(
            path,
            format!("internal placeholder regex failure: {source}"),
        )
    })?;
    if let Some(found) = placeholder_regex.find(rendered.as_str()) {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session '{session_id}' template has unknown placeholder '{}'",
                found.as_str()
            ),
        ));
    }

    if normalize_field(rendered.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session '{session_id}' resolved command is empty"),
        ));
    }
    Ok(rendered)
}

fn validate_tmux_target(
    target: RawTmuxTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<TmuxTarget, ConfigurationError> {
    if normalize_field(target.initial_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux initial-command must be non-empty"),
        ));
    }
    if normalize_field(target.resume_command.as_str()).is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux resume-command must be non-empty"),
        ));
    }

    if let Some(prompt_regex) = target.prompt_regex.as_deref() {
        if normalize_field(prompt_regex).is_empty() {
            return Err(ConfigurationError::invalid(
                coders_path,
                format!("coder '{coder_id}' tmux prompt-regex must be non-empty when set"),
            ));
        }
        compile_prompt_regex(prompt_regex, coders_path, coder_id, "tmux prompt-regex")?;
    }

    if matches!(target.prompt_inspect_lines, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' tmux prompt-inspect-lines must be greater than zero"),
        ));
    }

    Ok(TmuxTarget {
        initial_command: target.initial_command,
        resume_command: target.resume_command,
        prompt_regex: target.prompt_regex,
        prompt_inspect_lines: target.prompt_inspect_lines,
        prompt_idle_column: target.prompt_idle_column,
    })
}

fn validate_acp_target(
    target: RawAcpTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<AcpTarget, ConfigurationError> {
    if matches!(target.turn_timeout_ms, Some(0)) {
        return Err(ConfigurationError::invalid(
            coders_path,
            format!("coder '{coder_id}' ACP turn-timeout-ms must be greater than zero"),
        ));
    }

    match target.channel {
        AcpChannel::Stdio => {
            let Some(command) = target.command.as_deref() else {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target requires non-empty command"),
                ));
            };
            if normalize_field(command).is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target requires non-empty command"),
                ));
            }
            if target.url.is_some() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target must not set url"),
                ));
            }
            if !target.headers.is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP stdio target must not set headers"),
                ));
            }
        }
        AcpChannel::Http => {
            let Some(url) = target.url.as_deref() else {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target requires non-empty url"),
                ));
            };
            if normalize_field(url).is_empty() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target requires non-empty url"),
                ));
            }
            if target.command.is_some() {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!("coder '{coder_id}' ACP http target must not set stdio-only fields"),
                ));
            }
            validate_name_value_entries(&target.headers, coders_path, coder_id, "headers")?;
        }
    }

    validate_name_value_entries(&target.environment, coders_path, coder_id, "environment")?;

    Ok(AcpTarget {
        channel: target.channel,
        command: target.command,
        url: target.url,
        turn_timeout_ms: target.turn_timeout_ms,
        headers: target.headers,
        environment: target.environment,
    })
}

fn validate_name_value_entries(
    entries: &[NameValueEntry],
    path: &Path,
    coder_id: &str,
    field_name: &str,
) -> Result<(), ConfigurationError> {
    for (index, entry) in entries.iter().enumerate() {
        if normalize_field(entry.name.as_str()).is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("coder '{coder_id}' {field_name} entry {index} has empty name"),
            ));
        }
        if normalize_field(entry.value.as_str()).is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("coder '{coder_id}' {field_name} entry {index} has empty value"),
            ));
        }
    }
    Ok(())
}

fn prompt_readiness_from_tmux_target(
    target: &TmuxTarget,
    path: &Path,
    session_id: &str,
) -> Result<Option<PromptReadinessTemplate>, ConfigurationError> {
    let Some(prompt_regex) = target.prompt_regex.as_deref() else {
        return Ok(None);
    };
    compile_prompt_regex(prompt_regex, path, session_id, "prompt-regex")?;
    Ok(Some(PromptReadinessTemplate {
        prompt_regex: prompt_regex.to_string(),
        inspect_lines: target.prompt_inspect_lines,
        input_idle_cursor_column: target.prompt_idle_column,
    }))
}

fn compile_prompt_regex(
    pattern: &str,
    path: &Path,
    session_id: &str,
    field_name: &str,
) -> Result<(), ConfigurationError> {
    Regex::new(pattern).map(|_| ()).map_err(|source| {
        ConfigurationError::invalid(
            path,
            format!("invalid {field_name} for session/coder '{session_id}': {source}"),
        )
    })
}

fn normalize_field(value: &str) -> &str {
    value.trim()
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_session_id(path: &Path, session_id: &str) -> Result<(), ConfigurationError> {
    let mut characters = session_id.chars();
    let Some(first) = characters.next() else {
        return Err(ConfigurationError::invalid(
            path,
            "session id must be non-empty",
        ));
    };
    if !first.is_ascii_alphabetic() {
        return Err(ConfigurationError::invalid(
            path,
            format!("session id '{session_id}' must start with an ASCII alphabetic character"),
        ));
    }
    if !characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(ConfigurationError::invalid(
            path,
            format!(
                "session id '{session_id}' may only contain ASCII alphanumeric characters, '-' or '_'"
            ),
        ));
    }
    if session_id.len() > SESSION_ID_LENGTH_MAX {
        return Err(ConfigurationError::invalid(
            path,
            format!("session id '{session_id}' exceeds max length {SESSION_ID_LENGTH_MAX}"),
        ));
    }
    Ok(())
}

/// Validates a global user session id in `session@GLOBAL` canonical form.
///
/// The `@GLOBAL` suffix is required; the local prefix follows the bundle
/// session-id grammar.
fn validate_global_session_id(path: &Path, session_id: &str) -> Result<(), ConfigurationError> {
    let Some(local) = session_id.strip_suffix(GLOBAL_SESSION_SUFFIX) else {
        return Err(ConfigurationError::invalid(
            path,
            format!("users session id '{session_id}' must be in 'session@GLOBAL' canonical form"),
        ));
    };
    if local.is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("users session id '{session_id}' has an empty local part"),
        ));
    }
    validate_session_id(path, local)
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(value) = fs::canonicalize(path) {
        return value;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(current_directory) = std::env::current_dir() {
        return current_directory.join(path);
    }
    path.to_path_buf()
}
