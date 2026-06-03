//! Relay-wide identity administration handlers (`new peer`, `change psk`).
//!
//! Unlike the per-bundle request handlers, these operate on the relay-level
//! principal store and are not bound to any bundle. The relay is the sole
//! authority for credential issuance (design D1b/D7): it generates the PSK,
//! stores only its hash, and returns the raw value (or writes it to an
//! operator-supplied path) exactly once.

use std::path::Path;

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::relay::authorization::{RelayActionFamily, authorize_relay_action};
use crate::relay::context::RequestPrincipal;
use crate::relay::identity::{
    IdentityIntrospectRights, PrincipalRecord, PrincipalStore, PrincipalType, canonical_session_id,
    classify_principal_id, generate_psk, hash_token_sha256, scope_permits, split_principal_id,
    write_psk_output_file,
};
use crate::relay::stream::{
    RelayStreamEvent, notify_trusted_hosts_of_revocation, revoke_streams_for_identity,
};
use crate::relay::{RelayError, RelayResponse, SCHEMA_VERSION, relay_error};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{peer_relay_psk_path, principal_store_path, session_identity_psk_path};

/// Inputs for a `new peer` registration.
pub(in crate::relay) struct NewPeerRequestContext {
    pub(in crate::relay) principal_id: String,
    pub(in crate::relay) scope: Option<String>,
    pub(in crate::relay) output_path: Option<String>,
}

/// Registers a new principal: generates a PSK, stores its hash, and returns the
/// raw PSK plus a config snippet (or writes the PSK to `output_path`).
pub(in crate::relay) fn handle_new_peer(
    configuration_root: &Path,
    state_root: &Path,
    requester_principal_id: &str,
    context: NewPeerRequestContext,
) -> Result<RelayResponse, RelayError> {
    authorize_relay_action(
        configuration_root,
        requester_principal_id,
        RelayActionFamily::New,
        "peer",
    )?;
    let principal_type = classify_target_principal(context.principal_id.as_str())?;
    let mut store = PrincipalStore::load(principal_store_path(state_root))?;
    store.prune_expired(OffsetDateTime::now_utc());
    if store
        .find_by_principal_id(context.principal_id.as_str())
        .is_some()
    {
        return Err(relay_error(
            "validation_principal_exists",
            "principal_id is already registered; rotate with change psk instead",
            Some(serde_json::json!({ "principal_id": context.principal_id })),
        ));
    }
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    store.insert(PrincipalRecord {
        principal_id: context.principal_id.clone(),
        principal_type,
        credential_hash,
        scope: context.scope.clone(),
        expires_at: None,
        metadata: Default::default(),
    });
    store.persist()?;

    let config_snippet =
        build_config_snippet(principal_type, context.principal_id.as_str(), state_root);
    let (returned_psk, written_path) = match context.output_path.as_deref() {
        Some(output_path) => {
            write_psk_output_file(Path::new(output_path), psk.as_str())?;
            (None, Some(output_path.to_string()))
        }
        None => (Some(psk), None),
    };
    Ok(RelayResponse::NewPeer {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id: context.principal_id,
        principal_type: principal_type.as_str().to_string(),
        psk: returned_psk,
        output_path: written_path,
        config_snippet,
    })
}

/// Rotates the PSK for an existing principal, preserving its type, scope, and
/// metadata. After the store update, any active connection that authenticated
/// with the old credential is force-disconnected: it receives a
/// `runtime_identity_revoked` error frame and its connection is closed, so a
/// rotated credential cannot keep a live session.
pub(in crate::relay) fn handle_change_psk(
    configuration_root: &Path,
    state_root: &Path,
    requester_principal_id: &str,
    principal_id: String,
) -> Result<RelayResponse, RelayError> {
    authorize_relay_action(
        configuration_root,
        requester_principal_id,
        RelayActionFamily::Change,
        "psk",
    )?;
    let mut store = PrincipalStore::load(principal_store_path(state_root))?;
    store.prune_expired(OffsetDateTime::now_utc());
    let Some(existing) = store.find_by_principal_id(principal_id.as_str()).cloned() else {
        return Err(relay_error(
            "validation_unknown_principal",
            "principal_id is not registered; create it with new peer first",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    };
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    store.remove_by_principal_id(principal_id.as_str());
    store.insert(PrincipalRecord {
        principal_id: principal_id.clone(),
        principal_type: existing.principal_type,
        credential_hash,
        scope: existing.scope,
        expires_at: existing.expires_at,
        metadata: existing.metadata,
    });
    store.persist()?;

    // Revoke any live connection still holding the rotated credential. The
    // store update alone keeps an already-authenticated session alive until it
    // reconnects; this force-disconnects it so rotation takes effect at once.
    let revoked_frame = RelayResponse::Error {
        error: relay_error(
            "runtime_identity_revoked",
            "identity credential was rotated; reconnect with the new credential",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ),
    };
    let revoked_connections = revoke_streams_for_identity(principal_id.as_str(), &revoked_frame);

    // Notify every connected trusted host whose scope covers the revoked
    // principal so they can drop any cached view of it. This is distinct from
    // the teardown above: the revoked principal's own session receives a typed
    // error frame, while watching hosts receive an `identity.revoked` event.
    let revoked_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let revoked_event = RelayStreamEvent {
        event_type: "identity.revoked".to_string(),
        // `bundle_name`/`target_session` are rewritten per recipient host by the
        // fan-out; the revoked principal is carried in the payload.
        bundle_name: String::new(),
        target_session: String::new(),
        created_at: revoked_at.clone(),
        payload: serde_json::json!({
            "principal_id": principal_id,
            "revoked_at": revoked_at,
        }),
    };
    let notified_hosts = notify_trusted_hosts_of_revocation(principal_id.as_str(), &revoked_event);
    emit_inscription(
        "relay.identity.psk_rotated",
        &serde_json::json!({
            "principal_id": principal_id,
            "revoked_connections": revoked_connections,
            "notified_hosts": notified_hosts,
        }),
    );

    Ok(RelayResponse::ChangePsk {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id,
        psk,
    })
}

/// Resolves an `IdentityIntrospect` request against the relay-wide principal
/// store, gated on the connection's recorded introspection rights.
///
/// Only an application principal carries `introspect_rights` (recorded at
/// Hello), and only targets within its registered scope may be introspected; a
/// connection without rights, or a target outside scope, receives an
/// authorization denial. The store is read without pruning so an expired
/// principal still surfaces (with `verified: false`) rather than vanishing.
pub(in crate::relay) fn handle_identity_introspect(
    state_root: &Path,
    principal: &RequestPrincipal,
    target_session: &str,
    bundle_name: Option<&str>,
) -> Result<RelayResponse, RelayError> {
    let target_principal_id = match bundle_name {
        Some(bundle) => canonical_session_id(target_session, bundle),
        None => target_session.to_string(),
    };
    let Some(rights) = principal.introspect_rights.as_ref() else {
        return Err(introspect_denied(target_principal_id.as_str()));
    };
    if !scope_permits(rights.scope.as_deref(), target_principal_id.as_str()) {
        return Err(introspect_denied(target_principal_id.as_str()));
    }
    let store = PrincipalStore::load(principal_store_path(state_root))?;
    let Some(record) = store.find_by_principal_id(target_principal_id.as_str()) else {
        return Err(relay_error(
            "validation_unknown_principal",
            "no registered principal matches the introspection target",
            Some(serde_json::json!({ "principal_id": target_principal_id })),
        ));
    };
    let verified = !record.is_expired(OffsetDateTime::now_utc());
    Ok(RelayResponse::IdentityIntrospect {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id: record.principal_id.clone(),
        expires_at: record.expires_at.clone(),
        on_behalf_of: None,
        verified,
    })
}

/// Builds the authorization denial returned for an introspection the connection
/// is not permitted to perform (no rights, or target outside scope).
fn introspect_denied(target_principal_id: &str) -> RelayError {
    relay_error(
        "authorization_forbidden",
        "request denied by authorization policy",
        Some(serde_json::json!({
            "capability": "identity.introspect",
            "target_principal_id": target_principal_id,
        })),
    )
}

/// Builds the `identity.snapshot` stream event delivered to a trusted-host
/// (application principal) connection right after Hello.
///
/// The snapshot carries the active (non-expired) principal records within the
/// host's registered scope, so the host can seed its identity view without an
/// initial introspect round-trip. Expired records are omitted (the snapshot is
/// the set of *active* principals); the host re-verifies any specific principal
/// through `IdentityIntrospect`. The store is loaded without pruning, but the
/// expiry filter keeps expired records out of the snapshot regardless.
pub(in crate::relay) fn build_identity_snapshot_event(
    state_root: &Path,
    host_principal_id: &str,
    rights: &IdentityIntrospectRights,
) -> Result<RelayStreamEvent, RelayError> {
    let store = PrincipalStore::load(principal_store_path(state_root))?;
    let now = OffsetDateTime::now_utc();
    let principals: Vec<Value> = store
        .records()
        .filter(|record| !record.is_expired(now))
        .filter(|record| scope_permits(rights.scope.as_deref(), record.principal_id.as_str()))
        .map(snapshot_principal_entry)
        .collect();
    // The host is a relay-wide principal with no bundle; label the event with
    // its namespace (e.g. `EXTERNAL`) rather than a bundle name.
    let namespace = split_principal_id(host_principal_id)
        .map(|(_, namespace)| namespace.to_string())
        .unwrap_or_default();
    Ok(RelayStreamEvent {
        event_type: "identity.snapshot".to_string(),
        bundle_name: namespace,
        target_session: host_principal_id.to_string(),
        created_at: now.format(&Rfc3339).unwrap_or_default(),
        payload: serde_json::json!({ "principals": principals }),
    })
}

/// Renders one active, in-scope principal record for the identity snapshot.
/// `expires_at` is omitted when the principal never expires, matching the
/// optional-field treatment in the introspect response.
fn snapshot_principal_entry(record: &PrincipalRecord) -> Value {
    let mut entry = serde_json::Map::new();
    entry.insert(
        "principal_id".to_string(),
        Value::String(record.principal_id.clone()),
    );
    if let Some(expires_at) = record.expires_at.as_ref() {
        entry.insert("expires_at".to_string(), Value::String(expires_at.clone()));
    }
    entry.insert("verified".to_string(), Value::Bool(true));
    Value::Object(entry)
}

/// Validates and classifies the target `principal_id` for registration.
fn classify_target_principal(principal_id: &str) -> Result<PrincipalType, RelayError> {
    classify_principal_id(principal_id).ok_or_else(|| {
        relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(serde_json::json!({ "principal_id": principal_id })),
        )
    })
}

/// Builds a human-facing snippet describing where the new credential is read
/// from at Hello time, keyed by principal type.
fn build_config_snippet(
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> String {
    match principal_type {
        PrincipalType::Session => match split_principal_id(principal_id) {
            Some((session_id, bundle_name)) => {
                let path = session_identity_psk_path(state_root, bundle_name, session_id);
                format!(
                    "Write the PSK to {} (mode 0600); the session presents it as identity_token at Hello.",
                    path.display()
                )
            }
            None => "Write the PSK to the session identity.psk path; the session presents it at Hello.".to_string(),
        },
        PrincipalType::Relay => {
            let alias = split_principal_id(principal_id)
                .map(|(local, _)| local)
                .unwrap_or(principal_id);
            let path = peer_relay_psk_path(state_root, alias);
            format!(
                "Store the PSK at {} (mode 0600) on the peer relay; it presents it as identity_token when connecting inbound.",
                path.display()
            )
        }
        PrincipalType::User | PrincipalType::Application => {
            "Store the PSK at an operator-chosen path (mode 0600) and present it as identity_token at Hello.".to_string()
        }
    }
}
