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

/// Validates a project name against the bundle-superset path-segment
/// grammar: ASCII alphanumerics plus `-`, `_`, and `.`, excluding the
/// exact `.` and `..` segments, with no length cap and no first-character
/// restriction. The grammar admits every path-safe canonical bundle id
/// while keeping substituted values free of shell metacharacters, quotes,
/// and whitespace.
pub(super) fn validate_project_name(
    value: &str,
    path: &Path,
    context: &str,
) -> Result<(), ConfigurationError> {
    let conforming = !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if conforming {
        return Ok(());
    }
    Err(ConfigurationError::invalid(
        path,
        format!(
            "{context} '{value}' must be a portable path segment: ASCII alphanumerics \
             plus '-', '_', or '.', excluding '.' and '..'"
        ),
    ))
}

/// Validates a bundle id against the shell-safe path-segment grammar: the
/// same grammar as project names, so a validated id substitutes raw into
/// command templates without shell metacharacters, quotes, or whitespace.
pub(super) fn validate_bundle_name(
    bundle_name: &str,
    path: &Path,
) -> Result<(), ConfigurationError> {
    validate_project_name(bundle_name, path, "bundle name")
}

/// Validates a project-name derivation rule name: exactly
/// `session-directory-basename` or `bundle-name`.
pub(super) fn validate_project_name_from(
    rule: &str,
    path: &Path,
    context: &str,
) -> Result<(), ConfigurationError> {
    if matches!(rule, "session-directory-basename" | "bundle-name") {
        return Ok(());
    }
    Err(ConfigurationError::invalid(
        path,
        format!("{context} '{rule}' must be 'session-directory-basename' or 'bundle-name'"),
    ))
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
