use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::relay::{RelayError, relay_error};
use crate::runtime::paths::{is_valid_bundle_name, session_identity_psk_path};

use super::store::PrincipalType;
use super::verify::split_principal_id;

/// Upper bound on a `config`-destination session-id component, mirroring
/// `configuration::SESSION_ID_LENGTH_MAX` (kept in sync manually since that
/// constant is crate-private to the configuration module).
const CONFIG_SESSION_ID_LENGTH_MAX: usize = 31;

/// Validates a caller-named `path` sink: it must be absolute, have a file name,
/// have an existing parent directory, and not be a symlinked target. Parent
/// directories are never created for a caller-named path.
pub(super) fn validate_output_path(
    path: &Path,
) -> Result<PathBuf, RelayError> {
    if !path.is_absolute() {
        return Err(invalid_output_path(
            path,
            "credential output path must be absolute",
        ));
    }
    if path.file_name().is_none() {
        return Err(invalid_output_path(
            path,
            "credential output path has no file name",
        ));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_output_path(
            path,
            "credential output path has no parent directory",
        ));
    };
    if !parent.is_dir() {
        return Err(invalid_output_path(
            path,
            "credential output parent directory does not exist",
        ));
    }
    // Preserve the no-follow contract: writing through a symlinked target is
    // refused. The temp+rename publish never opens the final path directly, so
    // this lstat check is the enforcement point for the caller-named sink.
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(invalid_output_path(
            path,
            "credential output path is a symlink",
        ));
    }
    Ok(path.to_path_buf())
}

/// Derives the relay-owned canonical credential path for a `config` sink.
///
/// Only session principals have a relay-owned credential location; peer-relay,
/// user, and application principals do not, so `config` is rejected for them.
/// The `<id>@<namespace>` components become filesystem path segments, so each is
/// checked against a safe-segment grammar to keep a crafted principal id from
/// escaping the state root.
pub(super) fn resolve_config_credential_path(
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> Result<PathBuf, RelayError> {
    if principal_type != PrincipalType::Session {
        return Err(relay_error(
            "validation_config_destination_unsupported",
            "config credential destination is only supported for session principals",
            Some(json!({
                "principal_id": principal_id,
                "principal_type": principal_type.as_str(),
            })),
        ));
    }
    let Some((session_id, bundle_name)) = split_principal_id(principal_id)
    else {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    // Enforce the real identity grammar, not merely path-safety: a config
    // destination must name an identity a configured session could actually own.
    // The session-id grammar (leading ASCII alpha, then alphanumeric/`-`/`_`)
    // and the namespace grammar both exclude `/`, `.`, and `..`, so this also
    // subsumes traversal rejection.
    if !is_valid_session_component(session_id)
        || !is_valid_namespace_component(bundle_name)
    {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "principal id components are not a valid session identity for a config destination",
            Some(json!({ "principal_id": principal_id })),
        ));
    }
    Ok(session_identity_psk_path(
        state_root,
        bundle_name,
        session_id,
    ))
}

/// True when `session_id` matches the configured session-id grammar
/// (`configuration::fields::validate_session_id`): non-empty, first character
/// ASCII alphabetic, remaining characters ASCII alphanumeric / `-` / `_`, within
/// the length bound. Rejects ids no configured session could own (e.g. `1worker`,
/// `worker.name`) and, by construction, `.` / `..` / path separators.
pub(super) fn is_valid_session_component(session_id: &str) -> bool {
    let mut characters = session_id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
        })
        && session_id.len() <= CONFIG_SESSION_ID_LENGTH_MAX
}

/// True when `namespace` is a valid bundle name usable as a path segment: it
/// satisfies the canonical bundle-name grammar (which permits `.`, so real
/// dotted bundle names like `team.one` are accepted) but is not a traversal-only
/// `.` / `..` segment. The grammar already excludes `/`, so this cannot escape
/// the state root.
pub(super) fn is_valid_namespace_component(namespace: &str) -> bool {
    is_valid_bundle_name(namespace) && namespace != "." && namespace != ".."
}

fn invalid_output_path(path: &Path, message: &str) -> RelayError {
    relay_error(
        "validation_invalid_output_path",
        message,
        Some(json!({ "path": path.display().to_string() })),
    )
}

/// Builds a `validation_invalid_credential_path` rejection naming the
/// symlinked ancestor component that aborted a state-root-owned write.
pub(crate) fn invalid_credential_path(
    component: &str,
    message: &str,
) -> RelayError {
    relay_error(
        "validation_invalid_credential_path",
        message,
        Some(json!({ "component": component })),
    )
}

/// Returns the state-root-relative form of a relay-owned path. Relay-owned
/// paths are constructed below the state root, so stripping always succeeds;
/// the panic documents that invariant rather than failing open.
pub(crate) fn state_relative_path<'a>(
    state_root: &Path,
    path: &'a Path,
) -> &'a Path {
    path.strip_prefix(state_root)
        .expect("relay-owned store path below the state root")
}
