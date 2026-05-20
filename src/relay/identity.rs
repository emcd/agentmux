use super::GLOBAL_SESSION_SUFFIX;

/// Returns the canonical `session@bundle` identity for a session id.
///
/// Global-user identities already carry the `@GLOBAL` suffix and are their own
/// canonical form; bundle-local identities are qualified with the bundle name.
pub(super) fn canonical_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    }
}

/// Returns the bundle-local session id for a possibly-canonical identity.
///
/// Strips a trailing `@{bundle_name}` qualifier so internal lookups match
/// configured member ids; global-user (`@GLOBAL`) identities and already-bare
/// ids are returned unchanged.
pub(super) fn bare_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return session_id.to_string();
    }
    let qualifier = format!("@{bundle_name}");
    session_id
        .strip_suffix(qualifier.as_str())
        .unwrap_or(session_id)
        .to_string()
}
