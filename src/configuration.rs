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

/// One configured bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleMember {
    pub session_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_readiness: Option<PromptReadinessTemplate>,
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

/// Configuration for one named bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConfiguration {
    pub schema_version: String,
    pub bundle_name: String,
    pub members: Vec<BundleMember>,
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
    initial_command: String,
    resume_command: String,
    #[serde(default)]
    prompt_regex: Option<String>,
    #[serde(default)]
    prompt_inspect_lines: Option<usize>,
    #[serde(default)]
    prompt_idle_column: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawBundleFile {
    format_version: u32,
    #[serde(default)]
    sessions: Vec<RawSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawSession {
    id: String,
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    directory: PathBuf,
    coder: String,
    #[serde(default)]
    coder_session_id: Option<String>,
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
            matches.push(member.session_name.clone());
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

        let session_name = normalize_field(session.name.as_str());
        if session_name.is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: "session name must be non-empty".to_string(),
            });
        }
        if !session_names.insert(session_name.to_string()) {
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
                    session_name, coder_id
                ),
            });
        };

        if session.directory.as_os_str().is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: bundle_path.to_path_buf(),
                message: format!("session '{}' directory must be non-empty", session_name),
            });
        }

        let coder_session_id = session
            .coder_session_id
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty());
        let command_template = if coder_session_id.is_some() {
            coder.resume_command.as_str()
        } else {
            coder.initial_command.as_str()
        };
        let start_command = render_command_template(
            command_template,
            coder_session_id,
            bundle_path,
            session_name,
        )?;

        let prompt_readiness = prompt_readiness_from_coder(coder, coders_path, session_name)?;
        members.push(BundleMember {
            session_name: session_name.to_string(),
            display_name: session
                .display_name
                .as_deref()
                .map(normalize_field)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            working_directory: Some(session.directory.clone()),
            start_command: Some(start_command),
            prompt_readiness,
        });
    }

    Ok(BundleConfiguration {
        schema_version: FORMAT_VERSION.to_string(),
        bundle_name: expected_bundle_name.to_string(),
        members,
    })
}

fn validate_coders(
    coders: Vec<RawCoder>,
    coders_path: &Path,
) -> Result<HashMap<String, RawCoder>, ConfigurationError> {
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

        if normalize_field(coder.initial_command.as_str()).is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: format!("coder '{}' initial-command must be non-empty", coder_id),
            });
        }
        if normalize_field(coder.resume_command.as_str()).is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: format!("coder '{}' resume-command must be non-empty", coder_id),
            });
        }

        if let Some(prompt_regex) = coder.prompt_regex.as_deref() {
            if normalize_field(prompt_regex).is_empty() {
                return Err(ConfigurationError::InvalidConfiguration {
                    path: coders_path.to_path_buf(),
                    message: format!(
                        "coder '{}' prompt-regex must be non-empty when set",
                        coder_id
                    ),
                });
            }
            compile_prompt_regex(prompt_regex, coders_path, coder_id, "prompt-regex")?;
        }

        if matches!(coder.prompt_inspect_lines, Some(0)) {
            return Err(ConfigurationError::InvalidConfiguration {
                path: coders_path.to_path_buf(),
                message: format!(
                    "coder '{}' prompt-inspect-lines must be greater than zero",
                    coder_id
                ),
            });
        }

        unique.insert(coder_id.to_string(), coder);
    }

    Ok(unique)
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
    session_name: &str,
) -> Result<String, ConfigurationError> {
    let mut rendered = template.to_string();

    if rendered.contains("{coder-session-id}") {
        let Some(coder_session_id) = coder_session_id else {
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: format!(
                    "session '{}' requires coder-session-id for template",
                    session_name
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
                session_name,
                found.as_str()
            ),
        });
    }

    if normalize_field(rendered.as_str()).is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("session '{}' resolved command is empty", session_name),
        });
    }
    Ok(rendered)
}

fn prompt_readiness_from_coder(
    coder: &RawCoder,
    path: &Path,
    session_name: &str,
) -> Result<Option<PromptReadinessTemplate>, ConfigurationError> {
    let Some(prompt_regex) = coder.prompt_regex.as_deref() else {
        return Ok(None);
    };
    compile_prompt_regex(prompt_regex, path, session_name, "prompt-regex")?;
    Ok(Some(PromptReadinessTemplate {
        prompt_regex: prompt_regex.to_string(),
        inspect_lines: coder.prompt_inspect_lines,
        input_idle_cursor_column: coder.prompt_idle_column,
    }))
}

fn compile_prompt_regex(
    pattern: &str,
    path: &Path,
    session_name: &str,
    field_name: &str,
) -> Result<(), ConfigurationError> {
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|source| ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("invalid {field_name} for session/coder '{session_name}': {source}"),
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
