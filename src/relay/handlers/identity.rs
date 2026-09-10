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

use crate::configuration::ConfigurationRoots;
use crate::relay::authorization::{RelayActionFamily, authorize_relay_action};
use crate::relay::context::RequestPrincipal;
use crate::relay::identity::{
    IdentityIntrospectRights, PrincipalRecord, PrincipalStore, PrincipalType,
    classify_principal_id, generate_psk, hash_token_sha256, parse_peer_scope, scope_permits,
    split_principal_id, stage_credential_sink, write_pending_credential,
};
use crate::relay::stream::{
    RelayStreamEvent, notify_trusted_hosts_of_revocation, revoke_streams_for_identity,
};
use crate::relay::{
    CredentialDestination, RelayDiagnostic, RelayError, RelayResponse, SCHEMA_VERSION, relay_error,
};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{peer_relay_psk_path, session_identity_psk_path};

/// Inputs for a `new peer` registration.
pub(in crate::relay) struct NewPeerRequestContext {
    pub(in crate::relay) principal_id: String,
    pub(in crate::relay) scope: Option<String>,
    pub(in crate::relay) destination: CredentialDestination,
}

/// Registers a new principal: generates a PSK, stores its hash, and either
/// returns the raw PSK or writes it to the requested credential destination
/// (a caller-named path or the principal's canonical config path).
///
/// The destination is validated and the credential written to a temp sibling
/// before the store is committed, so a rejected or unwritable destination
/// registers nothing; the temp file is published with an atomic rename after
/// the commit, and a failed rename rolls the just-inserted record back out.
pub(in crate::relay) fn handle_new_peer(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    context: NewPeerRequestContext,
) -> Result<RelayResponse, RelayError> {
    authorize_relay_action(
        configuration_roots,
        requester_principal_id,
        RelayActionFamily::New,
        "peer",
    )?;
    let principal_type = classify_target_principal(context.principal_id.as_str())?;
    let canonical_scope = if principal_type == PrincipalType::Relay {
        parse_peer_scope(context.scope.as_deref())?
    } else {
        context.scope.clone()
    };
    let mut store = PrincipalStore::load(state_root)?;
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
    let sink = stage_credential_sink(
        &context.destination,
        principal_type,
        context.principal_id.as_str(),
        state_root,
    )?;
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    let pending = write_pending_credential(&sink, psk.as_str(), state_root)?;
    store.insert(PrincipalRecord {
        principal_id: context.principal_id.clone(),
        principal_type,
        credential_hash,
        scope: canonical_scope.clone(),
        expires_at: None,
        metadata: Default::default(),
    });
    if let Err(error) = store.persist() {
        if !is_durability_uncertain(&error) {
            if let Some(pending) = pending {
                pending.abort();
            }
            return Err(error);
        }
        // Uncertain: the rename published the new record. Compensate by
        // removing it, but retain staged credential material until the
        // effective outcome is known.
        store.remove_by_principal_id(context.principal_id.as_str());
        match store.persist() {
            Ok(()) => {
                // Prior absence restored deterministically.
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            }
            Err(rollback) if is_durability_uncertain(&rollback) => {
                // Compensation rename succeeded: the prior absence is
                // effective now and only crash durability is uncertain.
                // Discard new material and report the original uncertainty —
                // a retry observes a clean unknown-principal.
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            }
            Err(rollback) => {
                // Compensation failed before its rename: the original
                // publication stands and the new record is effective. Publish
                // staged credential material best-effort so file sinks match
                // the effective record, then report the double fault for
                // operator reconciliation (introspect, drop, recreate). A
                // Response-destination PSK cannot be delivered alongside an
                // error and is lost here; nothing referenced it yet, so the
                // principal is recoverable.
                if let Some(pending) = pending {
                    let _ = pending.commit();
                }
                return Err(credential_rollback_failed(error, rollback));
            }
        }
    }

    let config_snippet =
        build_config_snippet(principal_type, context.principal_id.as_str(), state_root);
    let (returned_psk, written_path) = match pending {
        None => (Some(psk), None),
        Some(pending) => match pending.commit() {
            Ok(path) => (None, Some(path)),
            Err(error) if is_credential_uncertain(&error) => {
                // Post-rename directory-sync failure: the file is published
                // and the store record committed, so both stand. Rolling the
                // record back would orphan a published credential file with
                // no record; report the typed uncertainty instead.
                return Err(error);
            }
            Err(error) => {
                // The rename failed after the store commit: remove the record we
                // just inserted so no principal lingers without a credential
                // file, then surface the write failure. `persist` is atomic
                // (old store intact on failure), so a rollback failure means the
                // new hash is still published with no usable credential -- do not
                // swallow it.
                store.remove_by_principal_id(context.principal_id.as_str());
                if let Err(rollback) = store.persist() {
                    return Err(credential_rollback_failed(error, rollback));
                }
                return Err(error);
            }
        },
    };
    Ok(RelayResponse::NewPeer {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id: context.principal_id,
        principal_type: principal_type.as_str().to_string(),
        psk: returned_psk,
        written_path,
        config_snippet,
        diagnostics: scope_vocabulary_diagnostics(canonical_scope.as_deref()),
    })
}

/// Policy-tier values, which an ingress `scope` is not.
///
/// Session-policy controls take one of these tiers; an ingress scope is matched
/// literally against a `session@bundle` id or a bare namespace. They share no
/// values, so a scope spelled as a tier is almost certainly a confusion between
/// the two surfaces.
const POLICY_TIER_VALUES: [&str; 4] = ["none", "self", "home", "all"];

/// Flags an ingress `scope` spelled as a policy tier.
///
/// Advisory rather than a rejection: every one of these is a syntactically legal
/// namespace name, so a deployment could own a bundle called `all` for which the
/// scope is exactly right. A scope that merely resolves to nothing stays silent,
/// because peer credentials are routinely minted before the namespace they scope
/// exists, and a cross-relay scope may name a namespace this relay cannot see.
fn scope_vocabulary_diagnostics(scope: Option<&str>) -> Vec<RelayDiagnostic> {
    let Some(scope) = scope else {
        return Vec::new();
    };
    if !POLICY_TIER_VALUES.contains(&scope) {
        return Vec::new();
    }
    vec![RelayDiagnostic {
        code: "advisory_scope_resembles_policy_tier".to_string(),
        message: format!(
            "ingress scope '{scope}' is a policy-tier value, not an ingress scope; \
             an ingress scope names a session@bundle principal or a bare namespace, \
             so this matches a namespace literally named '{scope}' and nothing else"
        ),
    }]
}

/// Rotates the PSK for an existing principal, preserving its type, scope, and
/// metadata. After the store update, an active connection that authenticated
/// with the old credential is force-disconnected: it receives a
/// `runtime_identity_revoked` error frame and its connection is closed, so a
/// rotated credential cannot keep a live session.
///
/// A principal rotating its *own* credential is the exception: its connection is
/// left alive so the response carrying the new PSK can reach it. Trusted hosts
/// are notified either way, since a cached view of the credential is stale
/// regardless of who initiated the rotation.
pub(in crate::relay) fn handle_change_psk(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    principal_id: String,
    destination: CredentialDestination,
) -> Result<RelayResponse, RelayError> {
    authorize_relay_action(
        configuration_roots,
        requester_principal_id,
        RelayActionFamily::Change,
        "psk",
    )?;
    let mut store = PrincipalStore::load(state_root)?;
    store.prune_expired(OffsetDateTime::now_utc());
    let Some(existing) = store.find_by_principal_id(principal_id.as_str()).cloned() else {
        return Err(relay_error(
            "validation_unknown_principal",
            "principal_id is not registered; create it with new peer first",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    };
    // Validate and stage the destination before any store mutation so a rejected
    // destination neither rotates the credential nor revokes the live session.
    let sink = stage_credential_sink(
        &destination,
        existing.principal_type,
        principal_id.as_str(),
        state_root,
    )?;
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    let pending = write_pending_credential(&sink, psk.as_str(), state_root)?;
    store.remove_by_principal_id(principal_id.as_str());
    store.insert(PrincipalRecord {
        principal_id: principal_id.clone(),
        principal_type: existing.principal_type,
        credential_hash,
        scope: existing.scope.clone(),
        expires_at: existing.expires_at.clone(),
        metadata: existing.metadata.clone(),
    });
    if let Err(error) = store.persist() {
        if !is_durability_uncertain(&error) {
            // A pre-rename persist failure leaves the prior credential intact;
            // discard the staged write and do not revoke anything.
            if let Some(pending) = pending {
                pending.abort();
            }
            return Err(error);
        }
        // Uncertain: the rename published the rotated hash. Compensate by
        // restoring the prior record, retaining staged material until the
        // effective outcome is known.
        store.remove_by_principal_id(principal_id.as_str());
        store.insert(existing.clone());
        match store.persist() {
            Ok(()) => {
                // Prior record restored deterministically: the old credential
                // is effective again, so revocation stays skipped.
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            }
            Err(rollback) if is_durability_uncertain(&rollback) => {
                // Compensation rename succeeded: the prior record is
                // effective now and only crash durability is uncertain.
                // Discard new material, skip revocation, and report the
                // original uncertainty — the old credential authenticates and
                // a retry rotates cleanly.
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            }
            Err(rollback) => {
                // Compensation failed: the rotated hash may still be
                // effective. Fail closed — publish staged material
                // best-effort, tear down sessions holding the superseded
                // credential, notify watching hosts — and report the double
                // fault. A Response-destination PSK cannot be delivered
                // alongside an error and is lost here; the operator
                // reconciles by rotating again, which always applies cleanly
                // to the existing record.
                match pending {
                    None => {}
                    Some(pending) => {
                        let _ = pending.commit();
                    }
                }
                let (revoked_connections, notified_hosts) =
                    revoke_superseded_credential(principal_id.as_str(), requester_principal_id);
                emit_inscription(
                    "relay.identity.psk_rotate_failed",
                    &serde_json::json!({
                        "principal_id": principal_id,
                        "revoked_connections": revoked_connections,
                        "notified_hosts": notified_hosts,
                    }),
                );
                return Err(credential_rollback_failed(error, rollback));
            }
        }
    }
    let written_path = match pending {
        None => None,
        Some(pending) => match pending.commit() {
            Ok(path) => Some(path),
            Err(error) if is_credential_uncertain(&error) => {
                // Post-rename directory-sync failure: the rotated file is
                // published and the rotated hash committed, so both stand.
                // Restoring the prior record would leave the new file
                // authenticating nothing while the old hash returns. Tear
                // down sessions holding the superseded credential exactly as
                // the success path does, then report the typed uncertainty.
                let (revoked_connections, notified_hosts) =
                    revoke_superseded_credential(principal_id.as_str(), requester_principal_id);
                emit_inscription(
                    "relay.identity.psk_rotated",
                    &serde_json::json!({
                        "principal_id": principal_id,
                        "revoked_connections": revoked_connections,
                        "notified_hosts": notified_hosts,
                    }),
                );
                return Err(error);
            }
            Err(error) => {
                // The rename failed after the store commit: restore the prior
                // record so the unchanged config file still authenticates, and
                // leave live connections untouched. A rollback-persist failure
                // would leave the rotated hash published without a usable
                // credential, so surface it rather than swallowing it.
                store.remove_by_principal_id(principal_id.as_str());
                store.insert(existing);
                if let Err(rollback) = store.persist() {
                    return Err(credential_rollback_failed(error, rollback));
                }
                return Err(error);
            }
        },
    };

    // Revoke any live connection still holding the rotated credential. The
    // store update alone keeps an already-authenticated session alive until it
    // reconnects; this force-disconnects it so rotation takes effect at once.
    //
    // The requester's own connection is exempt. Its teardown signal is observed
    // by the connection's frame loop while this handler is still running on the
    // blocking pool, so the loop exits and discards the response this call is
    // about to return -- taking the only copy of the new PSK with it when the
    // destination is Response. Excluding it cannot leave some other session
    // alive on the prior credential: the stream registry keys one entry per
    // `principal_id`, so a self-rotation's only possible match is the requester.
    let (revoked_connections, notified_hosts) =
        revoke_superseded_credential(principal_id.as_str(), requester_principal_id);
    emit_inscription(
        "relay.identity.psk_rotated",
        &serde_json::json!({
            "principal_id": principal_id,
            "revoked_connections": revoked_connections,
            "notified_hosts": notified_hosts,
        }),
    );

    let returned_psk = if written_path.is_some() {
        None
    } else {
        Some(psk)
    };
    Ok(RelayResponse::ChangePsk {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id,
        psk: returned_psk,
        written_path,
    })
}

/// Replaces the ingress scope of a registered peer relay in place, preserving
/// its credential, identity, expiry, and unrelated metadata.
///
/// Gated on the dedicated `change.scope=all` control (rotation rights confer
/// none of it). The update runs under the shared identity-admin serialization,
/// so it orders against concurrent registration, rotation, drop, replacement,
/// and final ingress authorization decisions. An empty scope clears the grant;
/// repeating the normalized scope succeeds (performing directory
/// synchronization rather than short-circuiting) without rotating or
/// disconnecting the peer. Scope-only updates emit no credential revocation:
/// the peer stays connected and keeps its PSK.
///
/// Durability follows rename-publication semantics: the atomic rename is the
/// update linearization point for effective authority, and the parent-directory
/// sync completes durable success. A pre-rename failure leaves the old record
/// and effective grant intact; a post-rename directory-sync failure keeps the
/// published replacement effective and surfaces an
/// `internal_store_durability_uncertain` error carrying the effective scope
/// rather than success or an old-grant-intact claim.
pub(in crate::relay) fn handle_change_scope(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    principal_id: String,
    scope: String,
) -> Result<RelayResponse, RelayError> {
    authorize_relay_action(
        configuration_roots,
        requester_principal_id,
        RelayActionFamily::Change,
        "scope",
    )?;
    if classify_principal_id(principal_id.as_str()) != Some(PrincipalType::Relay) {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "change scope applies only to peer relay principals (<id>@RELAY)",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    }
    // The scope grammar is validated before the store lookup so malformed input
    // fails identically for missing and registered peers; record existence is
    // still disclosed only behind the authorization gate above.
    let canonical = parse_peer_scope(Some(scope.as_str()))?;
    // No expiry pruning here: this operation must not alter unrelated records
    // as a side effect.
    let mut store = PrincipalStore::load(state_root)?;
    let Some(existing) = store.find_by_principal_id(principal_id.as_str()).cloned() else {
        return Err(relay_error(
            "validation_unknown_principal",
            "principal_id is not registered; create it with new peer first",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    };
    if existing.principal_type != PrincipalType::Relay {
        return Err(relay_error(
            "validation_invalid_params",
            "change scope applies only to registered relay principals",
            Some(serde_json::json!({
                "field": "principal_id",
                "principal_id": principal_id,
            })),
        ));
    }
    let previous = existing.scope.clone();
    store.remove_by_principal_id(principal_id.as_str());
    store.insert(PrincipalRecord {
        principal_id: principal_id.clone(),
        principal_type: existing.principal_type,
        credential_hash: existing.credential_hash.clone(),
        scope: canonical.clone(),
        expires_at: existing.expires_at.clone(),
        metadata: existing.metadata.clone(),
    });
    if let Err(error) = store.persist() {
        if is_durability_uncertain(&error) {
            let effective = canonical.clone().unwrap_or_default();
            emit_inscription(
                "relay.identity.scope_updated",
                &serde_json::json!({
                    "actor": requester_principal_id,
                    "principal_id": principal_id,
                    "previous_scope": previous.unwrap_or_default(),
                    "scope": effective,
                    "durability": "uncertain",
                }),
            );
            return Err(relay_error(
                "internal_store_durability_uncertain",
                "scope replacement published but parent-directory sync failed; durability is uncertain and the replacement remains effective",
                Some(serde_json::json!({
                    "principal_id": principal_id,
                    "scope": effective,
                })),
            ));
        }
        return Err(error);
    }
    let effective = canonical.clone().unwrap_or_default();
    emit_inscription(
        "relay.identity.scope_updated",
        &serde_json::json!({
            "actor": requester_principal_id,
            "principal_id": principal_id,
            "previous_scope": previous.unwrap_or_default(),
            "scope": effective,
        }),
    );
    Ok(RelayResponse::ChangeScope {
        schema_version: SCHEMA_VERSION.to_string(),
        principal_id,
        scope: effective,
        diagnostics: scope_vocabulary_diagnostics(canonical.as_deref()),
    })
}

/// Deletes a principal from the relay-wide store and revokes it.
///
/// The store record is the only copy of the credential hash, so dropping a
/// principal permanently invalidates its credential. Every session bound to the
/// principal is then torn down, because a record that no longer exists must not
/// keep authenticating a live connection.
///
/// Credential files are left in place. Once the record is gone the file
/// authenticates nothing, and the relay cannot know where an operator
/// distributed it — for a peer relay it lives under the *connecting* relay's
/// state root, which this relay cannot see.
pub(in crate::relay) fn handle_drop_peer(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    principal_id: String,
) -> Result<RelayResponse, RelayError> {
    // Ahead of the authorization gate, unlike the unknown-principal check below.
    // This compares the request against the requester's own identity and reads
    // nothing privileged, so it cannot disclose anything the caller does not
    // already know, and a caller dropping their own id gets this answer even
    // when they hold no grant at all.
    if principal_id == requester_principal_id {
        return Err(relay_error(
            "validation_self_drop_forbidden",
            "a principal cannot drop itself; drop it from another authenticated principal",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    }
    authorize_relay_action(
        configuration_roots,
        requester_principal_id,
        RelayActionFamily::Drop,
        "peer",
    )?;
    let mut store = PrincipalStore::load(state_root)?;
    store.prune_expired(OffsetDateTime::now_utc());
    // Behind the gate: answering this for an unauthorized caller would disclose
    // whether an arbitrary principal exists.
    let Some(existing) = store.find_by_principal_id(principal_id.as_str()).cloned() else {
        return Err(relay_error(
            "validation_unknown_principal",
            "principal_id is not registered",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ));
    };
    store.remove_by_principal_id(principal_id.as_str());
    // Persist before revoking: a pre-rename persist failure leaves the
    // principal authenticating and nothing is torn down. A post-rename
    // durability failure already published the removal, so revocation still
    // runs (the record is effectively gone) and the uncertainty is reported
    // instead of success.
    let durability_uncertain = match store.persist() {
        Ok(()) => None,
        Err(error) if is_durability_uncertain(&error) => Some(error),
        Err(error) => return Err(error),
    };

    let revoked_frame = RelayResponse::Error {
        error: relay_error(
            "runtime_identity_revoked",
            "identity was dropped; the credential no longer authenticates",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ),
    };
    let revoked_connections = revoke_streams_for_identity(principal_id.as_str(), &revoked_frame);
    let revoked_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let revoked_event = RelayStreamEvent {
        event_type: "identity.revoked".to_string(),
        target_session: String::new(),
        created_at: revoked_at.clone(),
        payload: serde_json::json!({
            "principal_id": principal_id,
            "revoked_at": revoked_at,
        }),
    };
    let notified_hosts = notify_trusted_hosts_of_revocation(principal_id.as_str(), &revoked_event);
    emit_inscription(
        "relay.identity.principal_dropped",
        &serde_json::json!({
            "principal_id": principal_id,
            "principal_type": existing.principal_type.as_str(),
            "revoked_connections": revoked_connections,
            "notified_hosts": notified_hosts,
        }),
    );

    let credential_path =
        relay_owned_credential_path(existing.principal_type, principal_id.as_str(), state_root);
    let principal_type = existing.principal_type.as_str().to_string();
    match durability_uncertain {
        None => Ok(RelayResponse::DropPeer {
            schema_version: SCHEMA_VERSION.to_string(),
            credential_path,
            principal_id,
            principal_type,
        }),
        Some(_) => Err(relay_error(
            "internal_store_durability_uncertain",
            "principal removal published but parent-directory sync failed; durability is uncertain and the removal remains effective",
            Some(serde_json::json!({ "principal_id": principal_id })),
        )),
    }
}

/// Renders the relay-owned canonical credential path for a dropped principal,
/// or `None` where the relay owns no such location.
///
/// Session principals only. A peer relay's credential lives under the
/// *connecting* relay's state root, and user/application principals store theirs
/// at an operator-chosen path, so in both cases a path derived from this relay's
/// state root would name a file that is not the operator's credential.
fn relay_owned_credential_path(
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> Option<String> {
    if principal_type != PrincipalType::Session {
        return None;
    }
    let (session_id, namespace) = split_principal_id(principal_id)?;
    Some(
        session_identity_psk_path(state_root, namespace, session_id)
            .display()
            .to_string(),
    )
}

/// Resolves an `IdentityIntrospect` request against the relay-wide principal
/// store, gated on the connection's recorded introspection rights.
///
/// `target_session` must be a qualified principal id (`<id>@<namespace>`);
/// bare ids are rejected with `validation_invalid_params` before any
/// authorization check. Only an application principal carries
/// `introspect_rights` (recorded at Hello), and only targets within its
/// registered scope may be introspected; a connection without rights, or a
/// target outside scope, receives an authorization denial. The store is read
/// without pruning so an expired principal still surfaces (with
/// `verified: false`) rather than vanishing.
pub(in crate::relay) fn handle_identity_introspect(
    state_root: &Path,
    principal: &RequestPrincipal,
    target_session: &str,
) -> Result<RelayResponse, RelayError> {
    // Format validation precedes the authorization gate: a malformed target is
    // a field error regardless of the connection's introspection rights.
    if split_principal_id(target_session).is_none() {
        return Err(relay_error(
            "validation_invalid_params",
            "target_session must be a qualified principal id (<id>@<namespace>)",
            Some(serde_json::json!({ "field": "target_session" })),
        ));
    }
    let Some(rights) = principal.introspect_rights.as_ref() else {
        return Err(introspect_denied(target_session));
    };
    if !scope_permits(rights.scope.as_deref(), target_session) {
        return Err(introspect_denied(target_session));
    }
    let store = PrincipalStore::load(state_root)?;
    let Some(record) = store.find_by_principal_id(target_session) else {
        return Err(relay_error(
            "validation_unknown_principal",
            "no registered principal matches the introspection target",
            Some(serde_json::json!({ "principal_id": target_session })),
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
    let store = PrincipalStore::load(state_root)?;
    let now = OffsetDateTime::now_utc();
    let principals: Vec<Value> = store
        .records()
        .filter(|record| !record.is_expired(now))
        .filter(|record| scope_permits(rights.scope.as_deref(), record.principal_id.as_str()))
        .map(snapshot_principal_entry)
        .collect();
    Ok(RelayStreamEvent {
        event_type: "identity.snapshot".to_string(),
        // The host is a relay-wide principal; its full principal id already
        // carries the namespace suffix (e.g. `@EXTERNAL`).
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

/// True when a store-persist error reports post-rename durability uncertainty
/// rather than a pre-publication failure. Pre-rename failures leave the old
/// store intact; uncertain ones already published the replacement, so callers
/// must compensate (or complete the effective state) instead of assuming the
/// old record survived.
fn is_durability_uncertain(error: &RelayError) -> bool {
    error.code == "internal_store_durability_uncertain"
}

/// True when a credential-sink commit error reports post-rename durability
/// uncertainty rather than a pre-publication failure. The file is published
/// and the store record committed, so callers must preserve both instead of
/// compensating the store.
fn is_credential_uncertain(error: &RelayError) -> bool {
    error.code == "internal_credential_durability_uncertain"
}

/// Tears down live sessions holding a superseded credential and notifies
/// watching trusted hosts, returning `(revoked_connections, notified_hosts)`.
///
/// Shared by the rotation success path and the compensation-failure path, where
/// the rotated hash may still be effective and sessions on the old credential
/// must not linger. The requester's own connection is exempt (see the rotation
/// success path for why excluding it is safe).
fn revoke_superseded_credential(
    principal_id: &str,
    requester_principal_id: &str,
) -> (usize, usize) {
    let revoked_frame = RelayResponse::Error {
        error: relay_error(
            "runtime_identity_revoked",
            "identity credential was rotated; reconnect with the new credential",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ),
    };
    let revoked_connections = if requester_principal_id == principal_id {
        0
    } else {
        revoke_streams_for_identity(principal_id, &revoked_frame)
    };

    // Notify every connected trusted host whose scope covers the revoked
    // principal so they can drop any cached view of it. This is distinct from
    // the teardown above: the revoked principal's own session receives a typed
    // error frame, while watching hosts receive an `identity.revoked` event.
    let revoked_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let revoked_event = RelayStreamEvent {
        event_type: "identity.revoked".to_string(),
        // `target_session` is rewritten per recipient host by the fan-out; the
        // revoked principal is carried in the payload.
        target_session: String::new(),
        created_at: revoked_at.clone(),
        payload: serde_json::json!({
            "principal_id": principal_id,
            "revoked_at": revoked_at,
        }),
    };
    let notified_hosts = notify_trusted_hosts_of_revocation(principal_id, &revoked_event);
    (revoked_connections, notified_hosts)
}

/// Builds the error for the double fault where a store publication left
/// durability uncertain *and* the compensating persist failed before its own
/// rename. The original publication therefore stands: the new record is
/// effective, and callers fail closed on it (revoking superseded sessions,
/// publishing staged material best-effort). A compensation that itself
/// publishes but stays sync-uncertain does NOT reach this error — the prior
/// state is effective then, so the original uncertainty is reported instead.
/// The operator reconciles by inspecting and re-rotating or recreating; staged
/// Response-destination secrets that could not be delivered are lost, but
/// nothing referenced them yet.
fn credential_rollback_failed(write_error: RelayError, rollback_error: RelayError) -> RelayError {
    relay_error(
        "internal_credential_rollback_failed",
        "credential publish failed and the principal-store rollback also failed; the store may be inconsistent",
        Some(serde_json::json!({
            "write_error": write_error.code,
            "rollback_error": rollback_error.code,
        })),
    )
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
            Some((session_id, namespace)) => {
                let path = session_identity_psk_path(state_root, namespace, session_id);
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
