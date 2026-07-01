//! Tmux session lifecycle primitives: create/query/prune managed sessions and
//! reap the tmux server when no managed sessions remain.
//!
//! These are the low-level building blocks the relay bundle orchestration
//! (reconcile/startup/shutdown, which stays in `relay/lifecycle.rs`) composes.
//! They surface a transport-local [`TmuxLifecycleError`]; relay maps it to its
//! `RelayError` envelope via a `From` impl on the relay side, so this module
//! never depends on `crate::relay`.

use std::{path::Path, thread, time::Duration};

use serde_json::{Value, json};

use crate::configuration::{BundleMember, TargetConfiguration};

use super::pane::{resolve_active_pane_target, run_tmux_command, run_tmux_command_capture};

const OWNERSHIP_OPTION_NAME: &str = "@agentmux_owned";
const OWNERSHIP_OPTION_VALUE: &str = "1";
const CREATE_MAX_ATTEMPTS: usize = 4;
const CREATE_RETRY_BASE_DELAY_MS: u64 = 35;
const CREATE_RETRY_JITTER_MS: u64 = 35;

/// Canonical error code the relay applies for tmux lifecycle failures. Carried
/// on [`TmuxLifecycleError`] so the relay `From` mapping and any in-tmux detail
/// payloads stay in lockstep.
pub(crate) const TMUX_LIFECYCLE_ERROR_CODE: &str = "internal_unexpected_failure";

/// A transport-local lifecycle failure. Mirrors the relay error shape (code,
/// message, structured details) without depending on `crate::relay`; the relay
/// maps it to `RelayError` at the orchestration boundary.
#[derive(Clone, Debug)]
pub(crate) struct TmuxLifecycleError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

fn tmux_lifecycle_error(code: &str, message: &str, details: Option<Value>) -> TmuxLifecycleError {
    TmuxLifecycleError {
        code: code.to_string(),
        message: message.to_string(),
        details,
    }
}

pub(crate) fn startup_tmux_member(
    tmux_socket: &Path,
    member: &BundleMember,
) -> Result<(), (String, String, Option<Value>)> {
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

pub(crate) fn create_member_with_retry(
    tmux_socket: &Path,
    member: &BundleMember,
) -> Result<(), TmuxLifecycleError> {
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
    Err(tmux_lifecycle_error(
        TMUX_LIFECYCLE_ERROR_CODE,
        "failed to create tmux session during reconciliation",
        Some(json!({
            "session_name": member.id,
            "cause": last_error.unwrap_or_else(|| "unknown tmux error".to_string())
        })),
    ))
}

fn create_member_once(tmux_socket: &Path, member: &BundleMember) -> Result<(), String> {
    let start_command = match &member.target {
        TargetConfiguration::Tmux(target) => target.start_command.as_str(),
        TargetConfiguration::Acp(_) | TargetConfiguration::Pty(_) => {
            return Err("cannot create tmux session for non-Tmux target".to_string());
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            return Err("cannot create tmux session for ui/pubsub target".to_string());
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

pub(crate) fn session_exists(tmux_socket: &Path, session_name: &str) -> Result<bool, String> {
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

pub(crate) fn prune_owned_session(
    tmux_socket: &Path,
    session_name: &str,
) -> Result<(), TmuxLifecycleError> {
    run_tmux_command(
        tmux_socket,
        &["kill-session", "-t", &format!("={session_name}")],
    )
    .map(|_| ())
    .map_err(|reason| {
        tmux_lifecycle_error(
            TMUX_LIFECYCLE_ERROR_CODE,
            "failed to prune agentmux-owned session",
            Some(json!({"session_name": session_name, "cause": reason})),
        )
    })
}

pub(crate) fn list_owned_sessions(tmux_socket: &Path) -> Result<Vec<String>, TmuxLifecycleError> {
    let output = match run_tmux_command_capture(
        tmux_socket,
        &["list-sessions", "-F", "#{session_name}\t#{@agentmux_owned}"],
    ) {
        Ok(output) => output,
        Err(reason) if is_missing_session_error(reason.as_str()) => return Ok(Vec::new()),
        Err(reason) => {
            return Err(tmux_lifecycle_error(
                TMUX_LIFECYCLE_ERROR_CODE,
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
        return Err(tmux_lifecycle_error(
            TMUX_LIFECYCLE_ERROR_CODE,
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

pub(crate) fn cleanup_tmux_server_when_unowned(
    tmux_socket: &Path,
) -> Result<bool, TmuxLifecycleError> {
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
            return Err(tmux_lifecycle_error(
                TMUX_LIFECYCLE_ERROR_CODE,
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
    Err(tmux_lifecycle_error(
        TMUX_LIFECYCLE_ERROR_CODE,
        "failed to clean up tmux socket",
        Some(json!({"cause": reason})),
    ))
}

fn list_all_sessions(tmux_socket: &Path) -> Result<Vec<String>, TmuxLifecycleError> {
    let output =
        match run_tmux_command_capture(tmux_socket, &["list-sessions", "-F", "#{session_name}"]) {
            Ok(output) => output,
            Err(reason) if is_missing_session_error(reason.as_str()) => return Ok(Vec::new()),
            Err(reason) => {
                return Err(tmux_lifecycle_error(
                    TMUX_LIFECYCLE_ERROR_CODE,
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
        return Err(tmux_lifecycle_error(
            TMUX_LIFECYCLE_ERROR_CODE,
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
