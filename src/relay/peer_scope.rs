//! Peer ingress scope grammar: `*` for every addressable namespace, explicit
//! comma-separated namespace sets, or empty/absent for no rights.
//!
//! Split from `identity` to keep that module under the line-count gate. The
//! application introspection scope helper (`scope_permits`) is unchanged and
//! never consulted here.

use std::path::Path;
use std::sync::MutexGuard;

use serde_json::json;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

use crate::runtime::paths::{is_valid_bundle_name, principal_store_path};

use super::context::PeerIngressAuthority;
use super::identity::{PrincipalStore, PrincipalType, split_principal_id};
use super::{RelayError, relay_error};

/// Returns true when `principal_id` names a peer relay principal (`<id>@RELAY`).
pub(crate) fn is_relay_principal_id(principal_id: &str) -> bool {
    matches!(principal_id.rsplit_once('@'), Some((_, "RELAY")))
}

/// Reserved namespace partitions that never carry addressable peer targets./// `RELAY` is the peer-identity namespace itself and `EXTERNAL` is the
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

/// Resolves the current authoritative ingress scope for an authenticated peer
/// relay connection.
///
/// Loads the principal store and returns the live record's scope only when the
/// record exists, is a relay principal, is unexpired, and still carries the
/// exact credential hash the connection presented at Hello. Any other outcome
/// — missing or dropped record, superseded credential (rotated or
/// re-registered under a reused id), expired record, or an unreadable store —
/// yields `None`, which ingress authorization treats as deny-by-default. The
/// credential-hash binding is what keeps a stale connection from inheriting a
/// replacement record's rights.
pub(crate) fn resolve_peer_ingress_scope(
    state_root: &Path,
    peer_principal_id: &str,
    credential_hash: Option<&str>,
) -> Option<String> {
    let presented = credential_hash?;
    let store = PrincipalStore::load(principal_store_path(state_root)).ok()?;
    let record = store.find_by_principal_id(peer_principal_id)?;
    if record.principal_type != PrincipalType::Relay {
        return None;
    }
    if record.is_expired(OffsetDateTime::now_utc()) {
        return None;
    }
    let stored = record.credential_hash.as_bytes();
    let candidate = presented.as_bytes();
    if stored.len() != candidate.len() || !bool::from(stored.ct_eq(candidate)) {
        return None;
    }
    record.scope.clone()
}

/// Live, serialized ingress authority for one peer request.
///
/// Resolves the current authoritative scope under the shared identity-admin
/// serialization and holds the guard across the caller's authorization and
/// local admission, so a scope update cannot interleave between the grant
/// check and admission. The scope is `None` (deny-by-default) when the record
/// is missing, superseded, expired, or unreadable.
pub(crate) struct LivePeerIngress<'a> {
    pub(crate) scope: Option<String>,
    _guard: MutexGuard<'a, ()>,
}

/// Acquires the shared serialization and resolves the peer's current grant.
///
/// Fails only when no ingress authority was threaded to the handler (a
/// programming error: every stream ingress path supplies one); all store-level
/// failures resolve to a deny-by-default `None` scope, never an error.
pub(crate) fn live_peer_ingress<'a>(
    authority: Option<PeerIngressAuthority<'a>>,
    peer_principal_id: &str,
    credential_hash: Option<&str>,
) -> Result<LivePeerIngress<'a>, RelayError> {
    let Some(authority) = authority else {
        return Err(relay_error(
            "internal_unexpected_request",
            "peer relay ingress without ingress authority",
            None,
        ));
    };
    let guard = authority
        .admin_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scope =
        resolve_peer_ingress_scope(authority.state_root, peer_principal_id, credential_hash);
    Ok(LivePeerIngress {
        scope,
        _guard: guard,
    })
}

// The credential-hash binding inside `resolve_peer_ingress_scope` is
// crate-private by design and unreachable through any public surface:
// revocation closes a dropped principal's connections before a stale one
// could present a superseded credential, so no public operation exercises the
// mismatch branch. This single inline test pins it alongside the grammar.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::identity::{PrincipalRecord, PrincipalType, hash_token_sha256};

    #[test]
    fn peer_scope_grammar_and_credential_binding() {
        // Grammar: wildcard, sets, canonicalization, and empty/no rights.
        assert_eq!(parse_peer_scope(None).unwrap(), None);
        assert_eq!(parse_peer_scope(Some("   ")).unwrap(), None);
        assert_eq!(parse_peer_scope(Some("*")).unwrap(), Some("*".to_string()));
        assert_eq!(
            parse_peer_scope(Some(" beta,alpha ,beta,GLOBAL ")).unwrap(),
            Some("GLOBAL,alpha,beta".to_string())
        );
        // Reserved and malformed names fail closed.
        for bad in [
            "alpha,,beta",
            "*,alpha",
            "alpha@bundle",
            "RELAY",
            "EXTERNAL",
            "has space",
            ".",
        ] {
            assert!(
                parse_peer_scope(Some(bad)).is_err(),
                "scope {bad:?} must be rejected"
            );
        }
        // Coverage: wildcard spans addressable namespaces only; sets are exact.
        assert!(peer_scope_covers_target(Some("*"), "alpha@bundle"));
        assert!(peer_scope_covers_target(Some("*"), "op@GLOBAL"));
        assert!(!peer_scope_covers_target(Some("*"), "x@RELAY"));
        assert!(!peer_scope_covers_target(Some("*"), "x@EXTERNAL"));
        assert!(peer_scope_covers_target(Some("alpha"), "s@alpha"));
        assert!(!peer_scope_covers_target(Some("alpha"), "s@beta"));
        assert!(!peer_scope_covers_target(None, "s@alpha"));
        assert!(peer_scope_covers_namespace(Some("*"), "future-type"));
        assert!(!peer_scope_covers_namespace(Some("*"), "RELAY"));

        // Binding: only the exact presented credential hash authorizes.
        let state_root =
            std::env::temp_dir().join(format!("agentmux-peer-scope-test-{}", std::process::id()));
        let token = "binding-probe-psk";
        let hash = hash_token_sha256(token);
        let mut store = PrincipalStore::load(principal_store_path(&state_root)).unwrap();
        store.insert(PrincipalRecord {
            principal_id: "probe@RELAY".to_string(),
            principal_type: PrincipalType::Relay,
            credential_hash: hash.clone(),
            scope: Some("*".to_string()),
            expires_at: None,
            metadata: Default::default(),
        });
        store.persist().unwrap();
        assert_eq!(
            resolve_peer_ingress_scope(&state_root, "probe@RELAY", Some(&hash)),
            Some("*".to_string())
        );
        assert_eq!(
            resolve_peer_ingress_scope(
                &state_root,
                "probe@RELAY",
                Some(&hash_token_sha256("other-psk"))
            ),
            None,
            "a superseded credential must not inherit the record"
        );
        assert_eq!(
            resolve_peer_ingress_scope(&state_root, "missing@RELAY", Some(&hash)),
            None
        );
        assert_eq!(
            resolve_peer_ingress_scope(&state_root, "probe@RELAY", None),
            None
        );
        std::fs::remove_dir_all(&state_root).ok();
    }
}
