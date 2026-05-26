use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use super::{ConfigurationError, GLOBAL_SESSION_SUFFIX, RESERVED_GROUP_ALL, SESSION_ID_LENGTH_MAX};

pub(super) fn validate_bundle_groups(
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

pub(super) fn validate_format_version(
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

pub(super) fn normalize_field(value: &str) -> &str {
    value.trim()
}

pub(super) fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn validate_session_id(path: &Path, session_id: &str) -> Result<(), ConfigurationError> {
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

/// Normalizes a global user session id to `session@GLOBAL` canonical form.
///
/// Operators may write either the bare local prefix (`user`) or the suffixed
/// form (`user@GLOBAL`); both are accepted and returned in canonical form. The
/// local prefix follows the bundle session-id grammar.
pub(super) fn normalize_global_session_id(
    path: &Path,
    session_id: &str,
) -> Result<String, ConfigurationError> {
    let local = session_id
        .strip_suffix(GLOBAL_SESSION_SUFFIX)
        .unwrap_or(session_id);
    if local.is_empty() {
        return Err(ConfigurationError::invalid(
            path,
            format!("users session id '{session_id}' has an empty local part"),
        ));
    }
    validate_session_id(path, local)?;
    Ok(format!("{local}{GLOBAL_SESSION_SUFFIX}"))
}

pub(super) fn canonicalize_best_effort(path: &Path) -> PathBuf {
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
