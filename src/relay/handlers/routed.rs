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

use crate::configuration::{BundleConfiguration, load_bundle_configuration};

use super::super::authorization::{AuthorizationContext, load_authorization_context};
use super::super::connection::BundleCatalog;
use super::super::{GLOBAL_NAMESPACE, RelayError, map_config, relay_error};

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
