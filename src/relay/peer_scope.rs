//! Peer ingress scope grammar: `*` for every addressable namespace, explicit
//! comma-separated namespace sets, or empty/absent for no rights.
//!
//! Split from `identity` to keep that module under the line-count gate. The
//! application introspection scope helper (`scope_permits`) is unchanged and
//! never consulted here.

use serde_json::json;

use crate::runtime::paths::is_valid_bundle_name;

use super::identity::split_principal_id;
use super::{RelayError, relay_error};

/// Reserved namespace partitions that never carry addressable peer targets.
///
/// `RELAY` is the peer-identity namespace itself and `EXTERNAL` is the
/// application namespace; neither names a bundle or `GLOBAL` principal set.
/// They are rejected as explicit peer-scope names and never covered by `*`.
const PEER_NON_ADDRESSABLE_NAMESPACES: [&str; 2] = ["RELAY", "EXTERNAL"];

/// Returns true when `namespace` can carry addressable peer targets.
///
/// Any syntactically valid namespace except the reserved relay/application
/// partitions qualifies, so newly introduced namespace types are covered
/// immediately without another grant update.
pub(crate) fn is_peer_addressable_namespace(namespace: &str) -> bool {
    !namespace.is_empty() && !PEER_NON_ADDRESSABLE_NAMESPACES.contains(&namespace)
}

/// Returns true when `name` is a syntactically valid explicit peer-scope name.
fn is_valid_peer_namespace_name(name: &str) -> bool {
    is_valid_bundle_name(name)
        && name != "."
        && name != ".."
        && !PEER_NON_ADDRESSABLE_NAMESPACES.contains(&name)
}

/// Parses a peer ingress `scope` into its canonical persisted form.
///
/// Grammar: `*` for every addressable namespace, a comma-separated set of
/// explicit namespaces, or empty/absent for no rights. Surrounding whitespace
/// is trimmed from the input and each item; sets are sorted, deduplicated,
/// case-preserved. `None` (absent) and whitespace-only input both mean no
/// rights (`Ok(None)`). Empty items, wildcard mixed with names, malformed
/// names, and reserved non-addressable partitions are `validation_invalid_params`
/// failures. Valid names need not exist yet.
pub(crate) fn parse_peer_scope(scope: Option<&str>) -> Result<Option<String>, RelayError> {
    let Some(raw) = scope else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed == "*" {
        return Ok(Some("*".to_string()));
    }
    let mut names: Vec<String> = Vec::new();
    for item in trimmed.split(',') {
        let name = item.trim();
        if name.is_empty() {
            return Err(relay_error(
                "validation_invalid_params",
                "peer scope contains an empty namespace name",
                Some(json!({ "field": "scope" })),
            ));
        }
        if name == "*" {
            return Err(relay_error(
                "validation_invalid_params",
                "peer scope wildcard cannot be combined with namespace names",
                Some(json!({ "field": "scope" })),
            ));
        }
        if !is_valid_peer_namespace_name(name) {
            return Err(relay_error(
                "validation_invalid_params",
                "peer scope names a malformed or non-addressable namespace",
                Some(json!({ "field": "scope", "namespace": name })),
            ));
        }
        names.push(name.to_string());
    }
    names.sort();
    names.dedup();
    Ok(Some(names.join(",")))
}

/// Whether a peer `scope` covers `target_principal_id`.
///
/// `*` covers every addressable namespace; a set covers its named namespaces;
/// empty/absent covers nothing. The application scope helper is unchanged and
/// never consulted here.
pub(crate) fn peer_scope_covers_target(scope: Option<&str>, target_principal_id: &str) -> bool {
    match scope {
        None => false,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return false;
            }
            if trimmed == "*" {
                return matches!(
                    split_principal_id(target_principal_id),
                    Some((_, namespace)) if is_peer_addressable_namespace(namespace)
                );
            }
            let Some((_, namespace)) = split_principal_id(target_principal_id) else {
                return false;
            };
            trimmed
                .split(',')
                .map(str::trim)
                .any(|name| name == namespace)
        }
    }
}

/// Whether a peer `scope` covers `namespace` for principal discovery.
pub(crate) fn peer_scope_covers_namespace(scope: Option<&str>, namespace: &str) -> bool {
    match scope {
        None => false,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return false;
            }
            if trimmed == "*" {
                return is_peer_addressable_namespace(namespace);
            }
            trimmed
                .split(',')
                .map(str::trim)
                .any(|name| name == namespace)
        }
    }
}
