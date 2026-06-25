use std::{collections::HashSet, path::Path, thread};

use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::configuration::{BundleConfiguration, TargetConfiguration, load_bundle_configuration};
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;

use super::identity::canonical_session_id;
use super::startup_state::note_session_served_successfully;
use super::stream::register_bundle_runtime_session;
use super::{
    BundleStartupReport, ListedSessionTransport, ReconciliationReport, RelayError, ShutdownReport,
    StartupFailureRecord, map_config, relay_error,
};
use crate::relay::authorization::{choices_pending_max, load_authorization_context};
use crate::relay::delivery::initialize_acp_target_for_startup;
use crate::tmux::lifecycle::{
    TmuxLifecycleError, cleanup_tmux_server_when_unowned, create_member_with_retry,
    list_owned_sessions, prune_owned_session, session_exists, startup_tmux_member,
};

impl From<TmuxLifecycleError> for RelayError {
    fn from(error: TmuxLifecycleError) -> Self {
        relay_error(error.code.as_str(), error.message.as_str(), error.details)
    }
}

/// Reconciles configured bundle sessions against tmux state.
///
/// # Errors
///
/// Returns structured validation/configuration errors when bundle loading
/// fails, and internal failures when tmux session operations fail.
pub(super) fn reconcile_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<ReconciliationReport, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let _authorization = load_authorization_context(configuration_root, Some(&bundle))?;
    reconcile_loaded_bundle(&bundle, tmux_socket)
}

/// Prunes managed sessions and reaps tmux server when safe during shutdown.
///
/// # Errors
///
/// Returns internal failures when tmux session operations fail.
pub(super) fn shutdown_bundle_runtime(tmux_socket: &Path) -> Result<ShutdownReport, RelayError> {
    let mut report = ShutdownReport::default();
    let mut owned_sessions = list_owned_sessions(tmux_socket)?;
    owned_sessions.sort();
    for session_name in owned_sessions {
        prune_owned_session(tmux_socket, &session_name)?;
        report.pruned_sessions.push(session_name);
    }
    report.killed_tmux_server = cleanup_tmux_server_when_unowned(tmux_socket)?;
    Ok(report)
}

pub(super) fn startup_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<BundleStartupReport, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, Some(&bundle))?;
    let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
    startup_loaded_bundle(
        &bundle,
        runtime_directory,
        tmux_socket.as_path(),
        choices_pending_max(&authorization),
    )
}

pub(super) fn reconcile_loaded_bundle(
    bundle: &BundleConfiguration,
    tmux_socket: &Path,
) -> Result<ReconciliationReport, RelayError> {
    let configured_sessions = bundle
        .members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();
    let mut missing = bundle
        .members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .filter_map(|member| match session_exists(tmux_socket, &member.id) {
            Ok(true) => None,
            Ok(false) => Some(Ok(member.clone())),
            Err(reason) => Some(Err(relay_error(
                "internal_unexpected_failure",
                "failed to query tmux session state during reconciliation",
                Some(json!({"session_name": member.id, "cause": reason})),
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    missing.sort_by(|left, right| left.id.cmp(&right.id));

    let mut report = ReconciliationReport::default();

    let mut stale_owned = list_owned_sessions(tmux_socket)?
        .into_iter()
        .filter(|session_name| !configured_sessions.contains(session_name))
        .collect::<Vec<_>>();
    stale_owned.sort();
    for session_name in stale_owned {
        prune_owned_session(tmux_socket, &session_name)?;
        report.pruned_sessions.push(session_name);
    }

    if let Some(bootstrap_member) = missing.first().cloned() {
        create_member_with_retry(tmux_socket, &bootstrap_member)?;
        report.bootstrap_session = Some(bootstrap_member.id.clone());
        report.created_sessions.push(bootstrap_member.id.clone());
    }

    let remaining = missing.into_iter().skip(1).collect::<Vec<_>>();
    if !remaining.is_empty() {
        let mut handles = Vec::with_capacity(remaining.len());
        for member in remaining {
            let tmux_socket = tmux_socket.to_path_buf();
            handles.push(thread::spawn(move || {
                create_member_with_retry(&tmux_socket, &member).map(|_| member.id.clone())
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(Ok(created_session)) => report.created_sessions.push(created_session),
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => {
                    return Err(relay_error(
                        "internal_unexpected_failure",
                        "reconciliation worker thread panicked",
                        None,
                    ));
                }
            }
        }
    }

    let _ = cleanup_tmux_server_when_unowned(tmux_socket)?;
    Ok(report)
}

fn startup_loaded_bundle(
    bundle: &BundleConfiguration,
    runtime_directory: &Path,
    tmux_socket: &Path,
    choices_pending_max: usize,
) -> Result<BundleStartupReport, RelayError> {
    let configured_tmux_sessions = bundle
        .members
        .iter()
        .filter(|member| matches!(member.target, TargetConfiguration::Tmux(_)))
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();

    let mut stale_owned = list_owned_sessions(tmux_socket)?
        .into_iter()
        .filter(|session_name| !configured_tmux_sessions.contains(session_name))
        .collect::<Vec<_>>();
    stale_owned.sort();
    for session_name in stale_owned {
        prune_owned_session(tmux_socket, &session_name)?;
    }

    let mut ready_session_count = 0usize;
    let mut failed_startups = Vec::<StartupFailureRecord>::new();
    let mut members = bundle.members.clone();
    members.sort_by(|left, right| left.id.cmp(&right.id));

    for member in members {
        // Register a static unified-registry entry for every configured coder
        // session before attempting startup, so look/raww can resolve the
        // target's capabilities and a not-yet-ready coder target is not mistaken
        // for an unknown one (the entry persists independently of transport
        // readiness). UI/Pubsub members have no managed runtime and register
        // dynamically at stream Hello.
        if matches!(
            member.target,
            TargetConfiguration::Tmux(_) | TargetConfiguration::Acp(_)
        ) {
            register_bundle_runtime_session(
                canonical_session_id(member.id.as_str(), bundle.bundle_name.as_str()).as_str(),
                bundle.bundle_name.as_str(),
                member.id.as_str(),
                member.target.session_type(),
            )
            .map_err(|error| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to register bundle-runtime session in the unified registry",
                    Some(json!({ "session_id": member.id, "cause": error.to_string() })),
                )
            })?;
        }
        match &member.target {
            TargetConfiguration::Tmux(_) => match startup_tmux_member(tmux_socket, &member) {
                Ok(()) => {
                    clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                    ready_session_count += 1;
                }
                Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                    session_id: member.id.clone(),
                    transport: ListedSessionTransport::Tmux,
                    code,
                    reason,
                    timestamp: startup_timestamp(),
                    sequence: 0,
                    details,
                }),
            },
            TargetConfiguration::Acp(_) => {
                match initialize_acp_target_for_startup(
                    bundle.bundle_name.as_str(),
                    runtime_directory,
                    &member,
                    choices_pending_max,
                ) {
                    Ok(()) => {
                        clear_session_startup_failures(runtime_directory, member.id.as_str())?;
                        ready_session_count += 1;
                    }
                    Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                        session_id: member.id.clone(),
                        transport: ListedSessionTransport::Acp,
                        code,
                        reason,
                        timestamp: startup_timestamp(),
                        sequence: 0,
                        details,
                    }),
                }
            }
            // `ui`/`pubsub` members have no implemented startup path; record a
            // structured startup failure and exclude them from active routing.
            TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
                failed_startups.push(StartupFailureRecord {
                    session_id: member.id.clone(),
                    transport: member.target.session_type().into(),
                    code: "runtime_session_type_not_implemented".to_string(),
                    reason: "session type delivery is not yet implemented".to_string(),
                    timestamp: startup_timestamp(),
                    sequence: 0,
                    details: None,
                });
            }
        }
    }

    Ok(BundleStartupReport {
        ready_session_count,
        failed_startups,
    })
}

fn clear_session_startup_failures(
    runtime_directory: &Path,
    session_id: &str,
) -> Result<(), RelayError> {
    note_session_served_successfully(runtime_directory, session_id).map_err(|reason| {
        relay_error(
            "internal_unexpected_failure",
            "failed to clear startup failure history after successful session startup",
            Some(json!({"session_id": session_id, "cause": reason})),
        )
    })
}

fn startup_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
