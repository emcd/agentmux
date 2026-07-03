//! Resolve_* / `*_error` helper cluster used by `serve_connection_frames`.
//!
//! Free-fn helpers for namespace routing, principal resolution, and error
//! shaping. Extracted from `connection.rs` (now `connection/mod.rs`) so the
//! state-machine file can stay focused on stream registration and frame
//! dispatch, with these pure helpers easy to read side-by-side. Every helper
//! is thread-safe and takes its inputs as arguments; the cluster holds no
//! internal state.

use std::path::Path;

use serde_json::{Value, json};
use time::OffsetDateTime;

use super::super::handlers::emit_choices_snapshot_for_ui_registration;
use super::super::identity::{
    PrincipalStore, PrincipalType, VerifiedIdentity, split_principal_id, verify_hello_credential,
};
use super::super::stream::StreamRegistration;
use super::{
    BundleCatalog, BundleRuntimePaths, HelloBinding, HelloFrame, RelayError, SCHEMA_VERSION,
};
use crate::relay::errors::{map_config, map_tui_config};
use crate::{
    configuration::{
        SessionType, load_bundle_configuration, load_policy_ids, load_tui_configuration,
    },
    relay::{errors::relay_error, identity::canonical_session_id},
    runtime::paths::principal_store_path,
};

/// Reconstructs the full `<id>@<namespace>` principal_id of the requester from
/// its stream registration. Session principals are stored bundle-local, so the
/// bound bundle is re-applied; relay-wide principals already carry their full
/// `principal_id`.
pub(super) fn full_requester_principal_id(registration: &StreamRegistration) -> String {
    match registration.namespace() {
        Some(namespace) => canonical_session_id(registration.requester_session_id(), namespace),
        None => registration.requester_session_id().to_string(),
    }
}

/// Resolves the subject bundle for a bundle-addressed operation (`Up`/`Down`,
/// choice decisions, and the `List` enumerate bundle) from the wire
/// `namespace` field and the connection's bound bundle.
///
/// This is the one remaining pre-handler bundle resolution. The target
/// operations (`Send`/`Look`/`Raww`) no longer use it: they are dispatched
/// through their namespace-centric paths, which resolve the requester in its home
/// namespace and load each target's bundle inside the handler. A bundle name does
/// a catalog lookup, `EXTERNAL`/`RELAY` are reserved for relay-internal routing
/// and rejected with `validation_unsupported_namespace`, and an absent namespace
/// falls back to the connection's bound bundle (a relay-wide connection with no
/// namespace is rejected). `List` with the `GLOBAL` namespace is handled before
/// this function and never reaches it.
pub(super) fn resolve_namespace_routing_bundle(
    bundle_catalog: &BundleCatalog,
    namespace: Option<&str>,
    bound_bundle: Option<&BundleRuntimePaths>,
) -> Result<BundleRuntimePaths, RelayError> {
    if let Some(namespace) = namespace {
        return match namespace {
            "EXTERNAL" | "RELAY" => Err(relay_error(
                "validation_unsupported_namespace",
                "namespace is reserved for relay-internal routing and cannot be selected by a client",
                Some(json!({ "namespace": namespace })),
            )),
            bundle_name => bundle_catalog
                .lookup(bundle_name)
                .ok_or_else(|| unknown_bundle_error(bundle_name)),
        };
    }
    if let Some(bound) = bound_bundle {
        return Ok(bound.clone());
    }
    Err(relay_error(
        "validation_missing_routing_namespace",
        "stream request from a relay-wide principal requires an explicit routing namespace",
        None,
    ))
}

pub(super) fn unknown_bundle_error(bundle_name: &str) -> RelayError {
    relay_error(
        "validation_unknown_bundle",
        "request target bundle is not configured on this relay",
        Some(json!({ "bundle_name": bundle_name })),
    )
}

pub(super) fn identity_claim_conflict_error(
    hello: &HelloFrame,
    existing_connection_id: Option<String>,
) -> RelayError {
    let mut details = serde_json::Map::new();
    details.insert(
        "principal_id".to_string(),
        Value::String(hello.principal_id.clone()),
    );
    details.insert(
        "reason".to_string(),
        Value::String("existing identity owner is still live".to_string()),
    );
    if let Some(value) = existing_connection_id {
        details.insert("existing_connection_id".to_string(), Value::String(value));
    }
    relay_error(
        "runtime_identity_claim_conflict",
        "stream identity is already claimed by a live connection",
        Some(Value::Object(details)),
    )
}

/// Emits choices snapshots to a freshly registered UI connection.
///
/// Session UI connections receive the snapshot for their bound bundle.
/// Relay-wide UI principals are not bundle-bound, so they replay every
/// configured bundle's snapshot — a global operator sees pending requests
/// across the whole relay on (re)connect.
pub(super) fn emit_registration_choices_snapshots(
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    binding: &HelloBinding,
) -> Result<(), RelayError> {
    match binding.bound_bundle.as_ref() {
        // Session principal: emit the snapshot for its bound bundle.
        Some(bundle_paths) => {
            if let Some((session_id, namespace)) = split_principal_id(binding.principal_id.as_str())
            {
                emit_choices_snapshot_for_ui_registration(
                    configuration_root,
                    namespace,
                    &bundle_paths.runtime_directory,
                    session_id,
                )?;
            }
        }
        // Relay-wide principal: not bundle-bound, so replay every configured
        // bundle's snapshot — a global operator sees pending requests across the
        // whole relay on (re)connect.
        None => {
            for bundle_paths in bundle_catalog.snapshot() {
                emit_choices_snapshot_for_ui_registration(
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    binding.principal_id.as_str(),
                )?;
            }
        }
    }
    Ok(())
}

/// Verifies the Hello credential, then resolves the connection binding from the
/// verified principal type.
///
/// Session principals (`@<bundle_name>`) do a bundle-catalog lookup and bind to
/// that bundle; non-session principals (`@GLOBAL`, `@EXTERNAL`, `@RELAY`) skip
/// the catalog and are not bundle-bound.
pub(super) fn resolve_hello_binding(
    configuration_root: &Path,
    state_root: &Path,
    bundle_catalog: &BundleCatalog,
    require_session_credentials: bool,
    hello: &HelloFrame,
) -> Result<HelloBinding, RelayError> {
    if hello.schema_version != SCHEMA_VERSION {
        return Err(relay_error(
            "validation_invalid_schema_version",
            "hello schema_version is not supported",
            Some(json!({
                "schema_version": hello.schema_version,
                "supported_schema_version": SCHEMA_VERSION,
            })),
        ));
    }
    let store = PrincipalStore::load(principal_store_path(state_root))?;
    // Expiry is enforced inside `verify_hello_credential` (against `now`) rather
    // than by pruning the store first: an expired-but-recognized credential is
    // rejected with the distinct `runtime_identity_expired` error and its
    // connection is closed, instead of being silently indistinguishable from an
    // unregistered one. The on-disk store is rewritten by startup/mutation
    // pruning, so this read-only path leaves the file untouched.
    let verified = verify_hello_credential(
        hello.principal_id.as_str(),
        hello.identity_token.as_str(),
        &store,
        require_session_credentials,
        OffsetDateTime::now_utc(),
    )?;
    let VerifiedIdentity {
        principal_type,
        store_backed,
        introspect_rights,
        ingress_scope,
    } = verified;
    match principal_type {
        PrincipalType::Session => {
            let (session_id, namespace) = split_principal_id(hello.principal_id.as_str())
                .ok_or_else(|| {
                    relay_error(
                        "validation_invalid_principal_id",
                        "session principal_id is not in <session>@<bundle> form",
                        Some(json!({ "principal_id": hello.principal_id })),
                    )
                })?;
            let bundle_paths = bundle_catalog
                .lookup(namespace)
                .ok_or_else(|| unknown_bundle_error(namespace))?;
            let session_type =
                resolve_bundle_member_session_type(configuration_root, namespace, session_id)?;
            Ok(HelloBinding {
                session_type,
                principal_id: hello.principal_id.clone(),
                bound_bundle: Some(bundle_paths),
                store_backed,
                introspect_rights,
                ingress_scope,
            })
        }
        PrincipalType::User => {
            let session_type =
                resolve_global_user_session_type(configuration_root, hello.principal_id.as_str())?;
            Ok(HelloBinding {
                session_type,
                principal_id: hello.principal_id.clone(),
                bound_bundle: None,
                store_backed,
                introspect_rights,
                ingress_scope,
            })
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(HelloBinding {
            session_type: SessionType::Pubsub,
            principal_id: hello.principal_id.clone(),
            bound_bundle: None,
            store_backed,
            introspect_rights,
            ingress_scope,
        }),
    }
}

/// Resolves the session type for a hello identity matching a bundle member.
pub(super) fn resolve_bundle_member_session_type(
    configuration_root: &Path,
    bundle_name: &str,
    session_id: &str,
) -> Result<SessionType, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let Some(member) = bundle.members.iter().find(|member| member.id == session_id) else {
        return Err(relay_error(
            "validation_unknown_sender",
            "hello session_id is not configured in associated bundle",
            Some(json!({
                "bundle_name": bundle.bundle_name,
                "session_id": session_id,
            })),
        ));
    };
    Ok(member.target.session_type())
}

/// Resolves the session type for a `@GLOBAL` user principal by searching
/// `users.toml` global users. Global users are not bundle-bound.
pub(super) fn resolve_global_user_session_type(
    configuration_root: &Path,
    principal_id: &str,
) -> Result<SessionType, RelayError> {
    let Some(users_configuration) =
        load_tui_configuration(configuration_root).map_err(map_tui_config)?
    else {
        return Err(global_user_missing_error(principal_id));
    };
    let Some(user_session) = users_configuration.session_by_id(principal_id) else {
        return Err(global_user_missing_error(principal_id));
    };
    let policy_ids = load_policy_ids(configuration_root).map_err(map_tui_config)?;
    if !policy_ids.contains(user_session.policy.as_str()) {
        return Err(relay_error(
            "validation_unknown_policy",
            "global user policy references unknown policy id",
            Some(json!({
                "session_id": user_session.id,
                "policy_id": user_session.policy,
            })),
        ));
    }
    Ok(user_session.session_type)
}

pub(super) fn global_user_missing_error(principal_id: &str) -> RelayError {
    relay_error(
        "validation_unknown_sender",
        "hello principal_id is not configured in global users",
        Some(json!({ "principal_id": principal_id })),
    )
}
