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

const FORMAT_VERSION: u32 = 1;
const CODERS_FILE: &str = "coders.toml";
const BUNDLES_DIRECTORY: &str = "bundles";
const BUNDLE_EXTENSION: &str = "toml";
pub const RESERVED_GROUP_ALL: &str = "ALL";

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
    pub target: TargetConfiguration,
    /// Optional persistent agent session handle sourced from
    /// `[[sessions]].coder-session-id` (not from `[[coders]]`).
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

/// Validated runtime target configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "transport", content = "config")]
pub enum TargetConfiguration {
    Tmux(TmuxTargetConfiguration),
    Acp(AcpTargetConfiguration),
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<NameValueEntry>,
}

/// Configuration for one named bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConfiguration {
    pub schema_version: String,
    pub bundle_name: String,
    pub groups: Vec<String>,
    pub members: Vec<BundleMember>,
}

/// Group membership metadata for one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleGroupMembership {
    pub bundle_name: String,
    pub groups: Vec<String>,
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
    headers: Vec<NameValueEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NameValueEntry {
    name: String,
    value: String,
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
    headers: Vec<NameValueEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawBundleFile {
    format_version: u32,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    sessions: Vec<RawSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawSession {
    id: String,
    #[serde(default)]
    name: Option<String>,
    directory: PathBuf,
    coder: String,
    #[serde(default)]
    coder_session_id: Option<String>,
    #[serde(default)]
    policy: Option<String>,
}

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
        validate_format_version(bundle_file.format_version, &bundle_path)?;
        if bundle_file.sessions.is_empty() {
            continue;
        }
        let groups = validate_bundle_groups(&bundle_file.groups, &bundle_path)?;
        memberships.push(BundleGroupMembership {
            bundle_name,
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
    validate_format_version(coders_file.format_version, coders_path)?;
    validate_format_version(bundle_file.format_version, bundle_path)?;

    let coders = validate_coders(coders_file.coders, coders_path)?;

    let groups = validate_bundle_groups(&bundle_file.groups, bundle_path)?;

    if bundle_file.sessions.is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: bundle_path.to_path_buf(),
            message: "sessions must contain at least one session".to_string(),
        });
    }

    let mut session_ids = HashSet::new();
    let mut session_names = HashSet::new();
    let mut members = Vec::with_capacity(bundle_file.sessions.len());

    for session in &bundle_file.sessions {
        let session_id = normalize_field(session.id.as_str());
        if session_id.is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: "session id must be non-empty".to_string(),
            });
        }
        if !session_ids.insert(session_id.to_string()) {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: format!("duplicate session id '{session_id}'"),
            });
        }

        let session_name = session
            .name
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty());
        if let Some(session_name) = session_name
            && !session_names.insert(session_name.to_string())
        {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: format!("duplicate session name '{session_name}'"),
            });
        }

        let coder_id = normalize_field(session.coder.as_str());
        let Some(coder) = coders.get(coder_id) else {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: format!(
                    "session '{}' references unknown coder '{}'",
                    session_id, coder_id
                ),
            });
        };

        if session.directory.as_os_str().is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: format!("session '{}' directory must be non-empty", session_id),
            });
        }

        let coder_session_id = session
            .coder_session_id
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty());
        let policy_id = session
            .policy
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let target = match &coder.target {
            CoderTarget::Tmux(target) => {
                let command_template = if coder_session_id.is_some() {
                    target.resume_command.as_str()
                } else {
                    target.initial_command.as_str()
                };
                let start_command = render_command_template(
                    command_template,
                    coder_session_id,
                    bundle_path,
                    session_id,
                )?;
                let prompt_readiness =
                    prompt_readiness_from_tmux_target(target, coders_path, session_id)?;
                TargetConfiguration::Tmux(TmuxTargetConfiguration {
                    start_command,
                    prompt_readiness,
                })
            }
            CoderTarget::Acp(target) => TargetConfiguration::Acp(AcpTargetConfiguration {
                channel: target.channel,
                command: target.command.clone(),
                url: target.url.clone(),
                headers: target.headers.clone(),
            }),
        };

        members.push(BundleMember {
            id: session_id.to_string(),
            name: session_name.map(ToString::to_string),
            working_directory: Some(session.directory.clone()),
            target,
            coder_session_id: coder_session_id.map(ToString::to_string),
            policy_id,
        });
    }

    Ok(BundleConfiguration {
        schema_version: FORMAT_VERSION.to_string(),
        bundle_name: expected_bundle_name.to_string(),
        groups,
        members,
    })
}

fn validate_coders(
    coders: Vec<RawCoder>,
    coders_path: &Path,
) -> Result<HashMap<String, Coder>, ConfigurationError> {
    if coders.is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: coders_path.to_path_buf(),
            message: "coders must contain at least one coder".to_string(),
        });
    }

    let mut unique = HashMap::new();
    for coder in coders {
        let coder_id = normalize_field(coder.id.as_str());
        if coder_id.is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: "coder id must be non-empty".to_string(),
            });
        }
        if unique.contains_key(coder_id) {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: format!("duplicate coder id '{coder_id}'"),
            });
        }

        let target = match (coder.tmux, coder.acp) {
            (Some(tmux), None) => {
                CoderTarget::Tmux(validate_tmux_target(tmux, coders_path, coder_id)?)
            }
            (None, Some(acp)) => CoderTarget::Acp(validate_acp_target(acp, coders_path, coder_id)?),
            (None, None) => {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' must define exactly one target table ([coders.tmux] or [coders.acp])",
                        coder_id
                    ),
                });
            }
            (Some(_), Some(_)) => {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' defines multiple target tables; expected exactly one",
                        coder_id
                    ),
                });
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

fn validate_format_version(version: u32, path: &Path) -> Result<(), ConfigurationError> {
    if version == FORMAT_VERSION {
        return Ok(());
    }
    Err(ConfigurationError::InvalidConfiguration {
        path: path.to_path_buf(),
        message: format!("unsupported format-version '{version}'"),
    })
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
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: format!(
                    "session '{}' requires coder-session-id for template",
                    session_id
                ),
            });
        };
        rendered = rendered.replace("{coder-session-id}", coder_session_id);
    }

    let placeholder_regex = Regex::new(r"\{[a-z][a-z0-9-]*\}").map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("internal placeholder regex failure: {source}"),
        }
    })?;
    if let Some(found) = placeholder_regex.find(rendered.as_str()) {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!(
                "session '{}' template has unknown placeholder '{}'",
                session_id,
                found.as_str()
            ),
        });
    }

    if normalize_field(rendered.as_str()).is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("session '{}' resolved command is empty", session_id),
        });
    }
    Ok(rendered)
}

fn validate_tmux_target(
    target: RawTmuxTarget,
    coders_path: &Path,
    coder_id: &str,
) -> Result<TmuxTarget, ConfigurationError> {
    if normalize_field(target.initial_command.as_str()).is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: coders_path.to_path_buf(),
            message: format!(
                "coder '{}' tmux initial-command must be non-empty",
                coder_id
            ),
        });
    }
    if normalize_field(target.resume_command.as_str()).is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: coders_path.to_path_buf(),
            message: format!("coder '{}' tmux resume-command must be non-empty", coder_id),
        });
    }

    if let Some(prompt_regex) = target.prompt_regex.as_deref() {
        if normalize_field(prompt_regex).is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: format!(
                    "coder '{}' tmux prompt-regex must be non-empty when set",
                    coder_id
                ),
            });
        }
        compile_prompt_regex(prompt_regex, coders_path, coder_id, "tmux prompt-regex")?;
    }

    if matches!(target.prompt_inspect_lines, Some(0)) {
        return Err(ConfigurationError::InvalidConfiguration {
            path: coders_path.to_path_buf(),
            message: format!(
                "coder '{}' tmux prompt-inspect-lines must be greater than zero",
                coder_id
            ),
        });
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
    match target.channel {
        AcpChannel::Stdio => {
            let Some(command) = target.command.as_deref() else {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' ACP stdio target requires non-empty command",
                        coder_id
                    ),
                });
            };
            if normalize_field(command).is_empty() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' ACP stdio target requires non-empty command",
                        coder_id
                    ),
                });
            }
            if target.url.is_some() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!("coder '{}' ACP stdio target must not set url", coder_id),
                });
            }
            if !target.headers.is_empty() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!("coder '{}' ACP stdio target must not set headers", coder_id),
                });
            }
        }
        AcpChannel::Http => {
            let Some(url) = target.url.as_deref() else {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' ACP http target requires non-empty url",
                        coder_id
                    ),
                });
            };
            if normalize_field(url).is_empty() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' ACP http target requires non-empty url",
                        coder_id
                    ),
                });
            }
            if target.command.is_some() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' ACP http target must not set stdio-only fields",
                        coder_id
                    ),
                });
            }
            validate_name_value_entries(&target.headers, coders_path, coder_id, "headers")?;
        }
    }

    Ok(AcpTarget {
        channel: target.channel,
        command: target.command,
        url: target.url,
        headers: target.headers,
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
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: format!(
                    "coder '{}' {} entry {} has empty name",
                    coder_id, field_name, index
                ),
            });
        }
        if normalize_field(entry.value.as_str()).is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: format!(
                    "coder '{}' {} entry {} has empty value",
                    coder_id, field_name, index
                ),
            });
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
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|source| ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("invalid {field_name} for session/coder '{session_id}': {source}"),
        })
}

fn normalize_field(value: &str) -> &str {
    value.trim()
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
