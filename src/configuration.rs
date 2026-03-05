//! Bundle configuration loading and sender-association helpers.

use std::{
    collections::HashSet,
    error::Error,
    fmt::{Display, Formatter},
    fs, io,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: &str = "1";
const BUNDLES_DIRECTORY: &str = "bundles";
const BUNDLE_EXTENSION: &str = "json";

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
}

/// Configuration for one named bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConfiguration {
    pub schema_version: String,
    pub bundle_name: String,
    pub members: Vec<BundleMember>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BundleConfigurationFile {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    bundle_name: Option<String>,
    members: Vec<BundleMember>,
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
    let path = bundle_configuration_path(configuration_root, bundle_name);
    if !path.exists() {
        return Err(ConfigurationError::UnknownBundle {
            bundle_name: bundle_name.to_string(),
            path,
        });
    }
    let raw = fs::read_to_string(&path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = serde_json::from_str::<BundleConfigurationFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.clone(),
            message: source.to_string(),
        }
    })?;
    validate_loaded_configuration(bundle_name, parsed, &path)
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
    parsed: BundleConfigurationFile,
    path: &Path,
) -> Result<BundleConfiguration, ConfigurationError> {
    if parsed.members.is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: "members must contain at least one session".to_string(),
        });
    }

    let bundle_name = parsed
        .bundle_name
        .unwrap_or_else(|| expected_bundle_name.to_string());
    if bundle_name != expected_bundle_name {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!(
                "bundle_name '{bundle_name}' does not match requested '{}'",
                expected_bundle_name
            ),
        });
    }

    let schema_version = parsed
        .schema_version
        .unwrap_or_else(|| SCHEMA_VERSION.to_string());
    if schema_version != SCHEMA_VERSION {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!("unsupported schema_version '{schema_version}'"),
        });
    }

    let mut names = HashSet::new();
    for member in &parsed.members {
        let session_name = member.session_name.trim();
        if session_name.is_empty() {
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: "member session_name must be non-empty".to_string(),
            });
        }
        if !names.insert(session_name.to_string()) {
            return Err(ConfigurationError::InvalidConfiguration {
                path: path.to_path_buf(),
                message: format!("duplicate session_name '{session_name}'"),
            });
        }
        validate_prompt_readiness(member, path)?;
    }

    Ok(BundleConfiguration {
        schema_version,
        bundle_name,
        members: parsed.members,
    })
}

fn validate_prompt_readiness(member: &BundleMember, path: &Path) -> Result<(), ConfigurationError> {
    let Some(template) = member.prompt_readiness.as_ref() else {
        return Ok(());
    };

    if template.prompt_regex.trim().is_empty() {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!(
                "prompt_readiness.prompt_regex must be non-empty for session '{}'",
                member.session_name
            ),
        });
    }
    compile_prompt_regex(
        &template.prompt_regex,
        path,
        member.session_name.as_str(),
        "prompt_regex",
    )?;
    if matches!(template.inspect_lines, Some(0)) {
        return Err(ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: format!(
                "prompt_readiness.inspect_lines must be greater than zero for session '{}'",
                member.session_name
            ),
        });
    }
    Ok(())
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
            message: format!(
                "invalid prompt_readiness.{field_name} for session '{session_name}': {source}"
            ),
        })
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
