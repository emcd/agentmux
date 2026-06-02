//! Relay-wide identity administration handlers (`new peer`, `change psk`).
//!
//! Unlike the per-bundle request handlers, these operate on the relay-level
//! principal store and are not bound to any bundle. The relay is the sole
//! authority for credential issuance (design D1b/D7): it generates the PSK,
//! stores only its hash, and returns the raw value (or writes it to an
//! operator-supplied path) exactly once.

use std::path::Path;

use time::OffsetDateTime;

use crate::relay::authorization::{RelayActionFamily, authorize_relay_action};
use crate::relay::identity::{
    PrincipalRecord, PrincipalStore, PrincipalType, classify_principal_id, generate_psk,
    hash_token_sha256, split_principal_id, write_psk_output_file,
};
use crate::relay::stream::revoke_streams_for_identity;
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
    emit_inscription(
        "relay.identity.psk_rotated",
        &serde_json::json!({
            "principal_id": principal_id,
            "revoked_connections": revoked_connections,
        }),
    );

    Ok(RelayResponse::ChangePsk {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id,
        psk,
    })
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
