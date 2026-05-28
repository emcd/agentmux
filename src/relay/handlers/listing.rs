use std::path::Path;

use serde_json::json;

use crate::{
    configuration::{BundleConfiguration, TargetConfiguration},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
};

use super::super::authorization::{AuthorizationContext, authorize_list};
use super::super::delivery::acp_session_ready_for_startup;
use super::super::lifecycle::{bundle_hosted, reconcile_loaded_bundle, shutdown_bundle_runtime};
use super::super::tmux::resolve_active_pane_target;
use super::super::{
    BundleTransitionEntry, ListedBundle, ListedBundleStartupHealth, ListedBundleState,
    ListedSession, RelayError, RelayResponse, SCHEMA_VERSION, canonical_session_id,
    load_startup_failures, relay_error,
};

pub(super) fn handle_bundle_up(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    let report = reconcile_loaded_bundle(bundle, tmux_socket.as_path())?;
    let changed = report.bootstrap_session.is_some()
        || !report.created_sessions.is_empty()
        || !report.pruned_sessions.is_empty();
    let bundle_result = if changed {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "hosted".to_string(),
            reason_code: None,
            reason: None,
        }
    } else {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "skipped".to_string(),
            reason_code: Some("already_hosted".to_string()),
            reason: Some("bundle runtime is already hosted".to_string()),
        }
    };
    Ok(RelayResponse::BundleTransition {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "up".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(changed),
        skipped_bundle_count: usize::from(!changed),
        failed_bundle_count: 0,
        changed_any: changed,
    })
}

pub(super) fn handle_bundle_down(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    let report = shutdown_bundle_runtime(tmux_socket.as_path())?;
    let changed = !report.pruned_sessions.is_empty() || report.killed_tmux_server;
    let bundle_result = if changed {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "unhosted".to_string(),
            reason_code: None,
            reason: None,
        }
    } else {
        BundleTransitionEntry {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "skipped".to_string(),
            reason_code: Some("already_unhosted".to_string()),
            reason: Some("bundle runtime is already unhosted".to_string()),
        }
    };
    Ok(RelayResponse::BundleTransition {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "down".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(changed),
        skipped_bundle_count: usize::from(!changed),
        failed_bundle_count: 0,
        changed_any: changed,
    })
}

pub(super) fn handle_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    sender_session: Option<String>,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    let sender_session = sender_session.ok_or_else(|| {
        relay_error(
            "validation_unknown_sender",
            "sender_session is required for list authorization",
            None,
        )
    })?;
    let sender = super::resolve_sender_identity(
        bundle,
        authorization,
        sender_session.as_str(),
        "sender_session",
    )?;
    authorize_list(bundle, authorization, sender.session_id.as_str())?;
    let sessions = bundle
        .members
        .iter()
        .map(|member| ListedSession {
            id: canonical_session_id(member.id.as_str(), bundle.bundle_name.as_str()),
            name: member.name.clone(),
            transport: member.target.session_type().into(),
        })
        .collect::<Vec<_>>();

    let recent_startup_failures = load_startup_failures(runtime_directory).map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to load startup failure history",
            Some(json!({
                "bundle_name": bundle.bundle_name,
                "cause": cause,
            })),
        )
    })?;
    let startup_failure_count = recent_startup_failures.len();
    let configured_session_count = bundle.members.len();
    let ready_session_count = bundle
        .members
        .iter()
        .filter(|member| {
            session_ready_for_list(bundle, runtime_directory, tmux_socket.as_path(), member)
        })
        .count();
    let (state, startup_health, state_reason_code, state_reason) =
        list_bundle_state(configured_session_count, ready_session_count);
    let hosted = bundle_hosted(bundle, tmux_socket.as_path())?;

    let response = RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle: ListedBundle {
            id: bundle.bundle_name.clone(),
            hosted,
            state,
            startup_health,
            state_reason_code,
            state_reason,
            startup_failure_count,
            recent_startup_failures,
            sessions,
        },
    };
    if let RelayResponse::List { bundle, .. } = &response {
        emit_inscription(
            "relay.list.response",
            &json!({
                "bundle_name": bundle.id,
                "sender_session": sender.session_id,
                "hosted": bundle.hosted,
                "state": bundle.state,
                "startup_health": bundle.startup_health,
                "startup_failure_count": bundle.startup_failure_count,
                "session_count": bundle.sessions.len(),
            }),
        );
    }
    Ok(response)
}

fn session_ready_for_list(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
    tmux_socket: &Path,
    member: &crate::configuration::BundleMember,
) -> bool {
    match member.target {
        TargetConfiguration::Tmux(_) => {
            resolve_active_pane_target(tmux_socket, member.id.as_str()).is_ok()
        }
        TargetConfiguration::Acp(_) => acp_session_ready_for_startup(
            bundle.bundle_name.as_str(),
            runtime_directory,
            member.id.as_str(),
        ),
        // `ui`/`pubsub` members have no implemented startup path; they are
        // never counted ready and surface a startup failure on bundle up.
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => false,
    }
}

fn list_bundle_state(
    configured_session_count: usize,
    ready_session_count: usize,
) -> (
    ListedBundleState,
    Option<ListedBundleStartupHealth>,
    Option<String>,
    Option<String>,
) {
    if configured_session_count == 0 {
        (
            ListedBundleState::Down,
            None,
            Some("runtime_no_configured_sessions".to_string()),
            Some("bundle has zero configured sessions".to_string()),
        )
    } else if ready_session_count == 0 {
        (
            ListedBundleState::Down,
            None,
            Some("runtime_startup_failed".to_string()),
            Some("zero configured sessions are currently ready".to_string()),
        )
    } else {
        let health = if ready_session_count == configured_session_count {
            ListedBundleStartupHealth::Healthy
        } else {
            ListedBundleStartupHealth::Degraded
        };
        (ListedBundleState::Up, Some(health), None, None)
    }
}
