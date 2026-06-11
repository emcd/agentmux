//! Shared namespace-centric dispatch helpers for the single-target operations
//! (`Look`, `Raww`).
//!
//! These mirror, for a single target, what `handle_send_routed` and
//! `assemble_delivery_groups` already do for `Send`: the requester is resolved
//! and authorized in its **home** namespace (its bound bundle, or `GLOBAL`), and
//! the target's bundle is loaded separately for existence validation and
//! delivery. No operation borrows a peer/target bundle as the requester's home.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::configuration::{
    BundleConfiguration, SessionType, load_bundle_configuration, load_tui_configuration,
};

use super::super::authorization::{
    AuthorizationContext, authorize_route, load_authorization_context,
};
use super::super::connection::BundleCatalog;
use super::super::routing::{OperationProfile, ResolvedRoute};
use super::super::{
    GLOBAL_NAMESPACE, RelayError, RelayResponse, map_config, map_tui_config, relay_error,
};

/// The dispatch spine for target operations.
///
/// It owns route resolution and authorization and fixes their order relative to
/// the operation's own work: `resolve` builds the [`ResolvedRoute`], `prepare`
/// validates target existence and loads delivery context, `authorize_route` runs
/// **between** them, and `execute` (the operation body) runs **only after**
/// authorization succeeds. The `prepare` and `execute` closures never call
/// `resolve_*` or `authorize_route` themselves — so the
/// existence-before-authorization ordering (`validation_unknown_target` before
/// `authorization_forbidden`) is structural, not a per-handler convention, and a
/// body cannot run on a route the spine has not authorized.
pub(super) fn run_target_operation<Prepared>(
    home_namespace: &str,
    authorization: &AuthorizationContext,
    profile: OperationProfile,
    resolve: impl FnOnce() -> Result<ResolvedRoute, RelayError>,
    prepare: impl FnOnce(&ResolvedRoute) -> Result<Prepared, RelayError>,
    execute: impl FnOnce(&ResolvedRoute, Prepared) -> Result<RelayResponse, RelayError>,
) -> Result<RelayResponse, RelayError> {
    let route = resolve()?;
    let prepared = prepare(&route)?;
    authorize_route(home_namespace, authorization, profile, &route)?;
    execute(&route, prepared)
}

/// Loads the requester's home authorization context. A bundle namespace loads the
/// bundle and its policy; the relay-wide `GLOBAL` namespace has no bundle and
/// resolves controls from the operator policy alone (`home_bundle` is `None`).
pub(super) fn load_home_context(
    home_namespace: &str,
    configuration_root: &Path,
) -> Result<(Option<BundleConfiguration>, AuthorizationContext), RelayError> {
    let home_bundle = if home_namespace == GLOBAL_NAMESPACE {
        None
    } else {
        Some(load_bundle_configuration(configuration_root, home_namespace).map_err(map_config)?)
    };
    let authorization = load_authorization_context(configuration_root, home_bundle.as_ref())?;
    Ok((home_bundle, authorization))
}

/// Resolves the declared session type of a relay-wide (`@GLOBAL`) target from
/// the global users configuration, for the look/raww capability check.
///
/// The capability source is configuration, not the live registry: a configured
/// target is checkable whether or not the session is connected. An
/// unconfigured principal is rejected with `validation_unknown_target`,
/// uniform with how a bundle target missing from bundle membership sorts
/// before authorization.
pub(super) fn resolve_relay_wide_target_session_type(
    configuration_root: &Path,
    target_principal_id: &str,
) -> Result<SessionType, RelayError> {
    let users_configuration = load_tui_configuration(configuration_root).map_err(map_tui_config)?;
    let Some(user_session) = users_configuration
        .as_ref()
        .and_then(|configuration| configuration.session_by_id(target_principal_id))
    else {
        return Err(relay_error(
            "validation_unknown_target",
            "target_session is not a configured relay-wide principal",
            Some(json!({ "target_session": target_principal_id })),
        ));
    };
    Ok(user_session.session_type)
}

/// Builds the error for a relay-wide target that passed the capability gate
/// but has no relay-wide operation path in the handler body.
///
/// Forward note from `add-transport-capability-flags`: unreachable via current
/// configuration (every `users.toml` session type is `Ui`, which fails the
/// gate first), but a capable relay-wide transport appearing there must fail
/// structurally rather than panic.
pub(super) fn relay_wide_operation_unimplemented(
    operation: &str,
    target_principal_id: &str,
    session_type: SessionType,
) -> RelayError {
    relay_error(
        "runtime_relay_wide_operation_not_implemented",
        "operation is not implemented for relay-wide targets",
        Some(json!({
            "operation": operation,
            "target_session": target_principal_id,
            "session_type": session_type,
        })),
    )
}

/// Loads the bundle hosting a single resolved target, with its runtime directory.
///
/// A same-namespace target reuses the already-loaded home bundle; a peer target
/// (or any target of a relay-wide requester, which has no home bundle) is loaded
/// from the catalog. An unconfigured target bundle is rejected with
/// `validation_unknown_bundle`.
pub(super) fn resolve_target_bundle(
    home_namespace: &str,
    home_bundle: Option<&BundleConfiguration>,
    home_runtime_directory: Option<&Path>,
    target_bundle_name: &str,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<(BundleConfiguration, PathBuf), RelayError> {
    if target_bundle_name == home_namespace
        && let Some(home_bundle) = home_bundle
    {
        let runtime = home_runtime_directory
            .map(Path::to_path_buf)
            .unwrap_or_default();
        return Ok((home_bundle.clone(), runtime));
    }
    let Some(paths) = bundle_catalog.lookup(target_bundle_name) else {
        return Err(relay_error(
            "validation_unknown_bundle",
            "target bundle is not configured on this relay",
            Some(json!({ "bundle_name": target_bundle_name })),
        ));
    };
    let bundle =
        load_bundle_configuration(configuration_root, target_bundle_name).map_err(map_config)?;
    Ok((bundle, paths.runtime_directory.clone()))
}
