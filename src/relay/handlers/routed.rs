//! Shared namespace-centric dispatch helpers for the single-target operations
//! (`Look`, `Raww`).
//!
//! These mirror, for a single target, what `handle_send_routed` and
//! `assemble_delivery_groups` already do for `Send`: the requester is resolved
//! and authorized in its **home** namespace (its bound bundle, or `GLOBAL`), and
//! the target's bundle is loaded separately for existence validation and
//! delivery. No operation borrows a peer/target bundle as the requester's home.

use std::path::PathBuf;

use serde_json::json;

use crate::configuration::{BundleConfiguration, ConfigurationRoots, load_bundle_configuration};

use super::super::authorization::{
    AuthorizationContext, RouteAuthorization, load_authorization_context,
};
use super::super::connection::BundleCatalog;
use super::super::lifecycle::{load_hosted_bundle, stamp_hosted_bundle};
use super::super::routing::{OperationProfile, ResolvedRoute};
use super::super::{
    GLOBAL_NAMESPACE, RELAY_NAMESPACE, RelayError, RelayResponse, map_config, relay_error,
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
    authorization: RouteAuthorization<'_>,
    profile: OperationProfile,
    resolve: impl FnOnce() -> Result<ResolvedRoute, RelayError>,
    prepare: impl FnOnce(&ResolvedRoute) -> Result<Prepared, RelayError>,
    execute: impl FnOnce(&ResolvedRoute, Prepared) -> Result<RelayResponse, RelayError>,
) -> Result<RelayResponse, RelayError> {
    let route = resolve()?;
    let prepared = prepare(&route)?;
    authorization.authorize(home_namespace, profile, &route)?;
    execute(&route, prepared)
}

/// Loads the requester's home authorization context. A bundle namespace loads the
/// bundle and its policy; the relay-wide `GLOBAL` and `RELAY` namespaces have no
/// bundle and resolve controls from the operator policy alone (`home_bundle` is
/// `None`). A `RELAY` (peer relay) requester is authorized by its ingress scope,
/// not this policy context, but still loads it so the shared spine can run.
pub(super) fn load_home_context(
    home_namespace: &str,
    configuration_roots: &ConfigurationRoots,
) -> Result<(Option<BundleConfiguration>, AuthorizationContext), RelayError> {
    let home_bundle = if home_namespace == GLOBAL_NAMESPACE || home_namespace == RELAY_NAMESPACE {
        None
    } else {
        Some(load_bundle_configuration(configuration_roots, home_namespace).map_err(map_config)?)
    };
    let authorization = load_authorization_context(configuration_roots, home_bundle.as_ref())?;
    Ok((home_bundle, authorization))
}

/// Loads the bundle hosting a single resolved target, with its runtime directory.
///
/// One rule for every target: the bundle must be in the relay's current catalog,
/// or the request is rejected with `validation_unknown_bundle`. A same-namespace
/// target then reuses the already-loaded home bundle rather than re-reading it;
/// a peer target (or any target of a relay-wide requester, which has no home
/// bundle) is loaded fresh. Both go through the hosting relay's paths, so the
/// state root reaching a member this delivery spawns does not depend on which
/// kind of target it was.
///
/// The catalog is authoritative here rather than the requester's bound paths,
/// which a connection holds as a clone from the time it bound. Those two
/// diverge: a failed watcher reload drops a bundle from the catalog while open
/// connections keep their copy. Serving a delivery from the stale copy would
/// spawn members for a bundle the relay is no longer hosting, so the absent
/// entry fails closed — the same way the Send path treats it.
pub(super) fn resolve_target_bundle(
    home_namespace: &str,
    home_bundle: Option<&BundleConfiguration>,
    target_namespace: &str,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
) -> Result<(BundleConfiguration, PathBuf), RelayError> {
    let Some(paths) = bundle_catalog.lookup(target_namespace) else {
        return Err(relay_error(
            "validation_unknown_bundle",
            "target bundle is not configured on this relay",
            Some(json!({ "bundle_name": target_namespace })),
        ));
    };
    let bundle = match home_bundle {
        Some(home_bundle) if target_namespace == home_namespace => {
            stamp_hosted_bundle(home_bundle.clone(), &paths)
        }
        _ => load_hosted_bundle(configuration_roots, &paths)?,
    };
    Ok((bundle, paths.runtime_directory.clone()))
}
