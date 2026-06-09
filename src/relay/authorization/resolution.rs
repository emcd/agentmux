//! Session resolution: map a requester (a bundle-member session or a relay-wide
//! principal) to its resolved [`PolicyControls`], plus the UI-session accessors
//! and the permission queue bound.

use std::{collections::HashMap, path::Path};

use serde_json::json;

use crate::{
    configuration::{
        BundleConfiguration, BundleMember, load_bundle_configuration, load_tui_configuration,
    },
    relay::{POLICIES_FILE, RelayError, relay_error},
};

use super::super::identity::{PrincipalType, classify_principal_id, split_principal_id};
use super::super::map_config;
use super::context::{AuthorizationContext, PolicyControls, PolicyScope};
use super::loading::{load_policy_presets, map_tui_configuration_error};

pub(in crate::relay) fn has_ui_session(
    authorization: &AuthorizationContext,
    session_id: &str,
) -> bool {
    authorization.ui_sessions.contains_key(session_id)
}

pub(in crate::relay) fn ui_session_display_name<'a>(
    authorization: &'a AuthorizationContext,
    session_id: &str,
) -> Option<&'a str> {
    authorization
        .ui_sessions
        .get(session_id)
        .and_then(|session| session.display_name.as_deref())
}

pub(in crate::relay) fn permission_max_pending(authorization: &AuthorizationContext) -> usize {
    authorization.permission_max_pending
}

pub(super) fn resolve_session_policy_controls<'a>(
    member: &BundleMember,
    presets: &'a HashMap<String, PolicyControls>,
    default_policy_id: Option<&str>,
    conservative_default: &'a PolicyControls,
    policies_path: &Path,
) -> Result<&'a PolicyControls, RelayError> {
    if let Some(policy_id) = member.policy_id.as_deref().and_then(normalize_policy_id) {
        return presets.get(policy_id).ok_or_else(|| {
            relay_error(
                "validation_invalid_arguments",
                "session policy references unknown policy id",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "session_id": member.id,
                    "policy_id": policy_id,
                })),
            )
        });
    }
    if let Some(default_policy_id) = default_policy_id {
        return presets.get(default_policy_id).ok_or_else(|| {
            relay_error(
                "validation_invalid_arguments",
                "authorization default policy references unknown policy id",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": default_policy_id,
                })),
            )
        });
    }
    Ok(conservative_default)
}

/// Resolves the policy controls for a requester identified by full
/// `principal_id`, independent of any bundle context.
///
/// `@GLOBAL` operators resolve their preset from the TUI configuration; session
/// principals resolve from their bundle member entry (falling back to the
/// default policy). Application and relay principals have no operator policy
/// mapping and resolve to the conservative default (which grants no relay-wide
/// action), keeping the path fail-closed.
pub(super) fn resolve_relay_principal_controls(
    configuration_root: &Path,
    requester_principal_id: &str,
) -> Result<PolicyControls, RelayError> {
    let (presets, default_policy_id) = load_policy_presets(configuration_root)?;
    let conservative_default = PolicyControls::conservative_default();
    let principal_type = classify_principal_id(requester_principal_id).ok_or_else(|| {
        relay_error(
            "validation_invalid_principal_id",
            "requester principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": requester_principal_id })),
        )
    })?;
    match principal_type {
        PrincipalType::User => {
            let Some(tui_configuration) =
                load_tui_configuration(configuration_root).map_err(map_tui_configuration_error)?
            else {
                return Ok(conservative_default);
            };
            let Some(session) = tui_configuration.session_by_id(requester_principal_id) else {
                return Ok(conservative_default);
            };
            let Some(policy_id) = normalize_policy_id(session.policy.as_str()) else {
                return Ok(conservative_default);
            };
            presets.get(policy_id).cloned().ok_or_else(|| {
                relay_error(
                    "validation_unknown_policy",
                    "global user policy references unknown policy id",
                    Some(json!({
                        "principal_id": requester_principal_id,
                        "policy_id": policy_id,
                    })),
                )
            })
        }
        PrincipalType::Session => {
            let Some((session_id, bundle_name)) = split_principal_id(requester_principal_id) else {
                return Ok(conservative_default);
            };
            let bundle =
                load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
            let Some(member) = bundle.members.iter().find(|member| member.id == session_id) else {
                return Ok(conservative_default);
            };
            let policies_path = configuration_root.join(POLICIES_FILE);
            resolve_session_policy_controls(
                member,
                &presets,
                default_policy_id.as_deref(),
                &conservative_default,
                policies_path.as_path(),
            )
            .cloned()
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(conservative_default),
    }
}

pub(in crate::relay) fn grant_authorized_ui_sessions(
    authorization: &AuthorizationContext,
    _bundle: &BundleConfiguration,
) -> Vec<String> {
    authorization
        .ui_sessions
        .keys()
        .filter(|session_id| {
            authorization
                .controls_by_session
                .get(session_id.as_str())
                .is_some_and(|controls| controls.grant.allows(PolicyScope::AllHome))
        })
        .cloned()
        .collect()
}

pub(super) fn controls_for_requester<'a>(
    authorization: &'a AuthorizationContext,
    dispatch_namespace: &str,
    requester_session: &str,
) -> Result<&'a PolicyControls, RelayError> {
    let controls = authorization
        .controls_by_session
        .get(requester_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "requester_session has no resolved policy controls",
                Some(json!({
                    "requester_session": requester_session,
                    "bundle_name": dispatch_namespace,
                })),
            )
        })?;
    let _ = controls.find;
    let _ = controls.do_controls.len();
    Ok(controls)
}

pub(super) fn normalize_policy_id(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}
