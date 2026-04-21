use std::{collections::HashSet, path::Path, thread, time::Duration};

use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::configuration::{BundleConfiguration, TargetConfiguration, load_bundle_configuration};

use super::{
    BundleStartupReport, ListedSessionTransport, ReconciliationReport, RelayError, ShutdownReport,
    StartupFailureRecord, map_config, relay_error,
};
use crate::relay::authorization::load_authorization_context;
use crate::relay::delivery::initialize_acp_target_for_startup;
use crate::relay::tmux::resolve_active_pane_target;
use crate::relay::tmux::{run_tmux_command, run_tmux_command_capture};

const OWNERSHIP_OPTION_NAME: &str = "@agentmux_owned";
const OWNERSHIP_OPTION_VALUE: &str = "1";
const CREATE_MAX_ATTEMPTS: usize = 4;
const CREATE_RETRY_BASE_DELAY_MS: u64 = 35;
const CREATE_RETRY_JITTER_MS: u64 = 35;

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
    let _authorization = load_authorization_context(configuration_root, &bundle)?;
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

pub(super) fn reconcile_loaded_bundle_for_lifecycle(
    bundle: &BundleConfiguration,
    tmux_socket: &Path,
) -> Result<ReconciliationReport, RelayError> {
    reconcile_loaded_bundle(bundle, tmux_socket)
}

pub(super) fn startup_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<BundleStartupReport, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let _authorization = load_authorization_context(configuration_root, &bundle)?;
    startup_loaded_bundle(&bundle, tmux_socket)
}

fn reconcile_loaded_bundle(
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
                Ok(Err(error)) => return Err(error),
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
    tmux_socket: &Path,
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
        match &member.target {
            TargetConfiguration::Tmux(_) => match startup_tmux_member(tmux_socket, &member) {
                Ok(()) => ready_session_count += 1,
                Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                    bundle_name: bundle.bundle_name.clone(),
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
                    tmux_socket,
                    &member,
                ) {
                    Ok(()) => ready_session_count += 1,
                    Err((code, reason, details)) => failed_startups.push(StartupFailureRecord {
                        bundle_name: bundle.bundle_name.clone(),
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
        }
    }

    Ok(BundleStartupReport {
        ready_session_count,
        failed_startups,
    })
}

fn startup_tmux_member(
    tmux_socket: &Path,
    member: &crate::configuration::BundleMember,
) -> Result<(), (String, String, Option<serde_json::Value>)> {
    match session_exists(tmux_socket, member.id.as_str()) {
        Ok(true) => {}
        Ok(false) => {
            if let Err(error) = create_member_with_retry(tmux_socket, member) {
                return Err((
                    "runtime_startup_failed".to_string(),
                    "failed to create tmux session during startup".to_string(),
                    Some(json!({
                        "session_id": member.id,
                        "cause": error.message,
                        "error_code": error.code,
                    })),
                ));
            }
        }
        Err(reason) => {
            return Err((
                "runtime_startup_failed".to_string(),
                "failed to query tmux session state during startup".to_string(),
                Some(json!({
                    "session_id": member.id,
                    "cause": reason,
                })),
            ));
        }
    }

    match resolve_active_pane_target(tmux_socket, member.id.as_str()) {
        Ok(_) => Ok(()),
        Err(reason) => Err((
            "runtime_startup_failed".to_string(),
            "tmux session is not ready".to_string(),
            Some(json!({
                "session_id": member.id,
                "cause": reason,
            })),
        )),
    }
}

fn startup_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn create_member_with_retry(
    tmux_socket: &Path,
    member: &crate::configuration::BundleMember,
) -> Result<(), RelayError> {
    let mut last_error = None::<String>;
    for attempt in 1..=CREATE_MAX_ATTEMPTS {
        match create_member_once(tmux_socket, member) {
            Ok(()) => return Ok(()),
            Err(reason) => {
                let transient = is_transient_tmux_error(reason.as_str());
                let retryable = transient && attempt < CREATE_MAX_ATTEMPTS;
                last_error = Some(reason);
                if retryable {
                    thread::sleep(retry_delay_for_attempt(&member.id, attempt));
                    continue;
                }
                break;
            }
        }
    }
    Err(relay_error(
        "internal_unexpected_failure",
        "failed to create tmux session during reconciliation",
        Some(json!({
            "session_name": member.id,
            "cause": last_error.unwrap_or_else(|| "unknown tmux error".to_string())
        })),
    ))
}

fn create_member_once(
    tmux_socket: &Path,
    member: &crate::configuration::BundleMember,
) -> Result<(), String> {
    let start_command = match &member.target {
        TargetConfiguration::Tmux(target) => target.start_command.as_str(),
        TargetConfiguration::Acp(_) => {
            return Err("cannot create tmux session for ACP target".to_string());
        }
    };

    let mut arguments = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        member.id.clone(),
    ];
    if let Some(working_directory) = member.working_directory.as_ref() {
        arguments.push("-c".to_string());
        arguments.push(working_directory.display().to_string());
    }
    arguments.push(start_command.to_string());
    run_tmux_command(tmux_socket, &arguments)?;
    run_tmux_command(
        tmux_socket,
        &[
            "set-option",
            "-t",
            member.id.as_str(),
            OWNERSHIP_OPTION_NAME,
            OWNERSHIP_OPTION_VALUE,
        ],
    )?;
    Ok(())
}

fn retry_delay_for_attempt(session_name: &str, attempt: usize) -> Duration {
    let hash = session_name
        .bytes()
        .fold(0u64, |value, byte| value.wrapping_add(u64::from(byte)));
    let jitter = (hash + (attempt as u64 * 7)) % CREATE_RETRY_JITTER_MS;
    Duration::from_millis((attempt as u64 * CREATE_RETRY_BASE_DELAY_MS) + jitter)
}

fn is_transient_tmux_error(reason: &str) -> bool {
    is_tmux_server_unavailable_error(reason)
}

fn is_tmux_server_unavailable_error(reason: &str) -> bool {
    let lowered = reason.to_ascii_lowercase();
    lowered.contains("no server running")
        || lowered.contains("failed to connect to server")
        || lowered.contains("server exited unexpectedly")
        || lowered.contains("connection refused")
        || lowered.contains("error connecting")
        || lowered.contains("no such file or directory")
}

fn session_exists(tmux_socket: &Path, session_name: &str) -> Result<bool, String> {
    let output = match run_tmux_command_capture(
        tmux_socket,
        &["has-session", "-t", &format!("={session_name}")],
    ) {
        Ok(output) => output,
        Err(reason) if is_missing_session_error(reason.as_str()) => return Ok(false),
        Err(reason) => return Err(reason),
    };
    if output.status.success() {
        return Ok(true);
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_missing_session_error(reason.as_str()) {
        return Ok(false);
    }
    if reason.is_empty() {
        return Err("tmux has-session failed".to_string());
    }
    Err(reason)
}

fn is_missing_session_error(reason: &str) -> bool {
    let lowered = reason.to_ascii_lowercase();
    lowered.contains("can't find session")
        || lowered.contains("no such file or directory")
        || lowered.contains("error connecting")
        || is_tmux_server_unavailable_error(reason)
}

fn prune_owned_session(tmux_socket: &Path, session_name: &str) -> Result<(), RelayError> {
    run_tmux_command(
        tmux_socket,
        &["kill-session", "-t", &format!("={session_name}")],
    )
    .map(|_| ())
    .map_err(|reason| {
        relay_error(
            "internal_unexpected_failure",
            "failed to prune agentmux-owned session",
            Some(json!({"session_name": session_name, "cause": reason})),
        )
    })
}

fn list_owned_sessions(tmux_socket: &Path) -> Result<Vec<String>, RelayError> {
    let output = match run_tmux_command_capture(
        tmux_socket,
        &["list-sessions", "-F", "#{session_name}\t#{@agentmux_owned}"],
    ) {
        Ok(output) => output,
        Err(reason) if is_missing_session_error(reason.as_str()) => return Ok(Vec::new()),
        Err(reason) => {
            return Err(relay_error(
                "internal_unexpected_failure",
                "failed to list tmux sessions",
                Some(json!({"cause": reason})),
            ));
        }
    };
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_missing_session_error(reason.as_str()) {
            return Ok(Vec::new());
        }
        return Err(relay_error(
            "internal_unexpected_failure",
            "failed to list tmux sessions",
            Some(json!({"cause": reason})),
        ));
    }
    let owned = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (session_name, marker) = line.split_once('\t').unwrap_or((line, ""));
            if marker.trim() == OWNERSHIP_OPTION_VALUE {
                return Some(session_name.to_string());
            }
            None
        })
        .collect::<Vec<_>>();
    Ok(owned)
}

fn cleanup_tmux_server_when_unowned(tmux_socket: &Path) -> Result<bool, RelayError> {
    if !list_owned_sessions(tmux_socket)?.is_empty() {
        return Ok(false);
    }
    if !list_all_sessions(tmux_socket)?.is_empty() {
        return Ok(false);
    }
    let output = match run_tmux_command_capture(tmux_socket, &["kill-server"]) {
        Ok(output) => output,
        Err(reason) if is_tmux_server_unavailable_error(reason.as_str()) => return Ok(false),
        Err(reason) => {
            return Err(relay_error(
                "internal_unexpected_failure",
                "failed to clean up tmux socket",
                Some(json!({"cause": reason})),
            ));
        }
    };
    if output.status.success() {
        return Ok(true);
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_tmux_server_unavailable_error(reason.as_str()) {
        return Ok(false);
    }
    Err(relay_error(
        "internal_unexpected_failure",
        "failed to clean up tmux socket",
        Some(json!({"cause": reason})),
    ))
}

fn list_all_sessions(tmux_socket: &Path) -> Result<Vec<String>, RelayError> {
    let output =
        match run_tmux_command_capture(tmux_socket, &["list-sessions", "-F", "#{session_name}"]) {
            Ok(output) => output,
            Err(reason) if is_missing_session_error(reason.as_str()) => return Ok(Vec::new()),
            Err(reason) => {
                return Err(relay_error(
                    "internal_unexpected_failure",
                    "failed to list tmux sessions",
                    Some(json!({"cause": reason})),
                ));
            }
        };
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_missing_session_error(reason.as_str()) {
            return Ok(Vec::new());
        }
        return Err(relay_error(
            "internal_unexpected_failure",
            "failed to list tmux sessions",
            Some(json!({"cause": reason})),
        ));
    }
    let sessions = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Ok(sessions)
}
