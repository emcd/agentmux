use serde_json::json;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::relay::{RelayError, relay_error};

use super::store::{PrincipalRecord, PrincipalStore, PrincipalType};

/// Returns true when a record's `expires_at` is absent-free but at or before
/// `now`, or is present yet unparseable. A record with no `expires_at` never
/// expires.
pub(super) fn record_is_expired(
    record: &PrincipalRecord,
    now: OffsetDateTime,
) -> bool {
    match record.expires_at.as_deref() {
        None => false,
        Some(raw) => match OffsetDateTime::parse(raw, &Rfc3339) {
            Ok(expires_at) => expires_at <= now,
            Err(_) => true,
        },
    }
}

/// Returns the lowercase hex SHA-256 digest of `token`.
pub(crate) fn hash_token_sha256(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Splits a `principal_id` of the form `<id>@<namespace>` into its components.
///
/// Returns `None` when the input lacks a single `@` separator or either side
/// is empty.
pub(crate) fn split_principal_id(principal_id: &str) -> Option<(&str, &str)> {
    let (local, namespace) = principal_id.rsplit_once('@')?;
    if local.is_empty() || namespace.is_empty() {
        return None;
    }
    Some((local, namespace))
}

/// Decides whether a host's registered `scope` permits introspecting (or
/// observing revocation of) `target_principal_id`.
///
/// The store records a single scope entry as either a canonical
/// `session_id@bundle_name` identity (exact match) or a bare `bundle_name`
/// (matches every session in that bundle namespace). A `None` scope is
/// fail-closed: no target is in scope.
pub(crate) fn scope_permits(
    scope: Option<&str>,
    target_principal_id: &str,
) -> bool {
    let Some(scope) = scope else {
        return false;
    };
    if scope == target_principal_id {
        return true;
    }
    matches!(
        split_principal_id(target_principal_id),
        Some((_, namespace)) if scope == namespace
    )
}

/// Classifies a `principal_id` by namespace partition.
///
/// `@GLOBAL` → user, `@EXTERNAL` → application, `@RELAY` → peer relay; any
/// other non-empty namespace is treated as a bundle-scoped session principal.
pub(crate) fn classify_principal_id(
    principal_id: &str,
) -> Option<PrincipalType> {
    let (_, namespace) = split_principal_id(principal_id)?;
    Some(match namespace {
        "GLOBAL" => PrincipalType::User,
        "EXTERNAL" => PrincipalType::Application,
        "RELAY" => PrincipalType::Relay,
        _ => PrincipalType::Session,
    })
}

/// Sentinel `identity_token` presented by sessions that have no provisioned
/// PSK file. Accepted for session and user principals only when relay-wide
/// credential enforcement is disabled (see `verify_hello_credential`).
pub(crate) const SOCKET_TRUST_TOKEN: &str = "socket-trust";

/// Introspection rights recorded on an application principal's connection at
/// Hello, so request dispatch can gate `IdentityIntrospect` on them.
///
/// Present only for `Application` principals (which always present a recognized
/// credential). The `scope` is the principal store record's `scope` field, set
/// at `new peer` registration for `@EXTERNAL` principals; `None` means the
/// principal was registered without a scope bound, which the dispatch gate
/// treats as fail-closed (no target is in scope).
#[derive(Clone, Debug)]
pub(crate) struct IdentityIntrospectRights {
    pub(crate) scope: Option<String>,
}

/// Result of verifying a Hello credential against the principal store.
pub(crate) struct VerifiedIdentity {
    pub(crate) principal_type: PrincipalType,
    /// True when a recognized store credential backed the identity; false for
    /// accepted `"socket-trust"` connections, which create no store entry.
    /// Drives sender-attribution (`authenticated_identity`) and distinguishes
    /// store-backed from socket-trust connections on the Hello path.
    pub(crate) store_backed: bool,
    /// Hex SHA-256 of the presented credential for store-backed connections;
    /// `None` for socket-trust. Recorded on the connection so live ingress
    /// authorization can verify the current record still carries this exact
    /// credential — a dropped/recreated or rotated record must not authorize a
    /// connection bound to a superseded credential merely because the
    /// principal id was reused.
    pub(crate) credential_hash: Option<String>,
    /// Introspection rights for an `Application` principal, carrying its
    /// registered scope; `None` for every other principal type. Recorded on the
    /// connection context so request dispatch can gate `IdentityIntrospect`
    /// (task 2.5).
    pub(crate) introspect_rights: Option<IdentityIntrospectRights>,
}

/// Verifies a Hello `principal_id` + `identity_token` against the principal
/// store and the relay-wide enforcement policy.
///
/// A recognized token must be registered to the claimed `principal_id`
/// (credential-to-identity binding). The `"socket-trust"` sentinel is accepted
/// for session and user principals only when enforcement is disabled;
/// application and relay principals always require a recognized credential.
/// Unrecognized non-sentinel tokens are rejected fail-closed regardless of
/// enforcement.
///
/// Expiry is detected here rather than by pruning the store before lookup: a
/// recognized credential whose record has expired is rejected with the distinct
/// `runtime_identity_expired` error (carrying `now`), so an expiring session is
/// told its credential lapsed rather than receiving the generic
/// `validation_unrecognized_credential`.
pub(crate) fn verify_hello_credential(
    principal_id: &str,
    identity_token: &str,
    store: &PrincipalStore,
    require_session_credentials: bool,
    now: OffsetDateTime,
) -> Result<VerifiedIdentity, RelayError> {
    let Some(claimed_type) = classify_principal_id(principal_id) else {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "hello principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    if identity_token == SOCKET_TRUST_TOKEN {
        return verify_socket_trust(
            principal_id,
            claimed_type,
            require_session_credentials,
        );
    }
    let hash = hash_token_sha256(identity_token);
    let Some(record) = store.find_by_credential_hash(&hash) else {
        return Err(relay_error(
            "validation_unrecognized_credential",
            "hello identity_token did not match any registered principal",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    if record.principal_id != principal_id {
        return Err(relay_error(
            "validation_identity_binding_mismatch",
            "presented credential is registered to a different principal_id",
            Some(json!({
                "principal_id": principal_id,
                "registered_principal_id": record.principal_id,
            })),
        ));
    }
    if record.is_expired(now) {
        return Err(relay_error(
            "runtime_identity_expired",
            "identity credential has expired; re-register or rotate the credential",
            Some(json!({
                "principal_id": principal_id,
                "expires_at": record.expires_at,
            })),
        ));
    }
    let introspect_rights = (record.principal_type
        == PrincipalType::Application)
        .then(|| IdentityIntrospectRights {
            scope: record.scope.clone(),
        });
    Ok(VerifiedIdentity {
        principal_type: record.principal_type,
        store_backed: true,
        credential_hash: Some(hash),
        introspect_rights,
    })
}

fn verify_socket_trust(
    principal_id: &str,
    claimed_type: PrincipalType,
    require_session_credentials: bool,
) -> Result<VerifiedIdentity, RelayError> {
    match claimed_type {
        PrincipalType::Application | PrincipalType::Relay => Err(relay_error(
            "validation_credential_required",
            "application and relay principals require a registered credential",
            Some(json!({ "principal_id": principal_id })),
        )),
        PrincipalType::Session | PrincipalType::User => {
            if require_session_credentials {
                return Err(relay_error(
                    "validation_credential_required",
                    "relay requires session credentials; socket-trust is not accepted",
                    Some(json!({ "principal_id": principal_id })),
                ));
            }
            Ok(VerifiedIdentity {
                principal_type: claimed_type,
                store_backed: false,
                // Socket-trust is accepted only for session and user
                // principals, never `Application` or `Relay`, so it grants no
                // introspect rights.
                credential_hash: None,
                introspect_rights: None,
            })
        },
    }
}
