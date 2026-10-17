use crate::relay::GLOBAL_SESSION_SUFFIX;

/// Returns the canonical `session@namespace` identity for a session id.
///
/// Qualification is idempotent: an id already carrying the namespace it would be
/// qualified with is its own canonical form and is returned unchanged. Global-user
/// identities are the familiar case, but not the only one — a peer relay
/// principal arrives at ingress already suffixed `@RELAY` and is then qualified
/// against the relay namespace, which without this yields `<id>@RELAY@RELAY`.
/// Bundle-local identities carry no suffix and are qualified.
pub(in crate::relay) fn canonical_session_id(
    session_id: &str,
    namespace: &str,
) -> String {
    let already_qualified = session_id.ends_with(GLOBAL_SESSION_SUFFIX)
        || session_id
            .strip_suffix(namespace)
            .is_some_and(|stem| stem.len() > 1 && stem.ends_with('@'));
    if already_qualified {
        session_id.to_string()
    } else {
        format!("{session_id}@{namespace}")
    }
}

/// Returns the bundle-local session id for a possibly-canonical identity.
///
/// Strips a trailing `@{namespace}` qualifier so internal lookups match
/// configured member ids; global-user (`@GLOBAL`) identities and already-bare
/// ids are returned unchanged.
pub(in crate::relay) fn bare_session_id(
    session_id: &str,
    namespace: &str,
) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return session_id.to_string();
    }
    let qualifier = format!("@{namespace}");
    session_id
        .strip_suffix(qualifier.as_str())
        .unwrap_or(session_id)
        .to_string()
}
