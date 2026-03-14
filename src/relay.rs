//! Relay IPC contract and message-routing implementation.

use std::{
    collections::HashSet,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{BundleConfiguration, ConfigurationError, load_bundle_configuration},
    envelope::{ENVELOPE_SCHEMA_VERSION, PromptBatchSettings},
    runtime::{inscriptions::emit_inscription, paths::BundleRuntimePaths},
};

mod authorization;
mod delivery;
mod tmux;

use self::authorization::{
    AuthorizationContext, authorize_list, authorize_look, authorize_send,
    load_authorization_context,
};
use self::delivery::{
    QuiescenceOptions, aggregate_chat_status, deliver_one_target, enqueue_async_delivery,
    prompt_batch_settings,
};
use self::tmux::{
    capture_pane_tail_lines, resolve_active_pane_target, run_tmux_command, run_tmux_command_capture,
};

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const OWNERSHIP_OPTION_NAME: &str = "@agentmux_owned";
const OWNERSHIP_OPTION_VALUE: &str = "1";
const CREATE_MAX_ATTEMPTS: usize = 4;
const CREATE_RETRY_BASE_DELAY_MS: u64 = 35;
const CREATE_RETRY_JITTER_MS: u64 = 35;
const DEFAULT_LOOK_LINES: usize = 120;
const MAX_LOOK_LINES: usize = 1000;
const POLICIES_FILE: &str = "policies.toml";
const POLICIES_FORMAT_VERSION: u32 = 1;

/// Recipient metadata returned by `list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Recipient {
    pub session_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Per-target delivery result for one `chat` request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChatResult {
    pub target_session: String,
    pub message_id: String,
    pub outcome: ChatOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Reconciliation results for one bundle lifecycle pass.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub bootstrap_session: Option<String>,
    pub created_sessions: Vec<String>,
    pub pruned_sessions: Vec<String>,
}

/// Managed-session cleanup results for relay shutdown.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShutdownReport {
    pub pruned_sessions: Vec<String>,
    pub killed_tmux_server: bool,
}

/// Aggregate delivery status for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Accepted,
    Success,
    Partial,
    Failure,
}

/// Per-target delivery outcome for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatOutcome {
    Queued,
    Delivered,
    Timeout,
    DroppedOnShutdown,
    Failed,
}

/// Chat delivery behavior requested by caller.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatDeliveryMode {
    Async,
    #[default]
    Sync,
}

/// Structured relay error object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Relay request protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RelayRequest {
    List {
        sender_session: Option<String>,
    },
    Chat {
        request_id: Option<String>,
        sender_session: String,
        message: String,
        targets: Vec<String>,
        broadcast: bool,
        #[serde(default)]
        delivery_mode: ChatDeliveryMode,
        #[serde(default)]
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        quiescence_timeout_ms: Option<u64>,
    },
    Look {
        requester_session: String,
        target_session: String,
        #[serde(default)]
        lines: Option<usize>,
        #[serde(default)]
        bundle_name: Option<String>,
    },
}

/// Relay response protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayResponse {
    List {
        schema_version: String,
        bundle_name: String,
        recipients: Vec<Recipient>,
    },
    Chat {
        schema_version: String,
        bundle_name: String,
        request_id: Option<String>,
        sender_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_display_name: Option<String>,
        delivery_mode: ChatDeliveryMode,
        status: ChatStatus,
        results: Vec<ChatResult>,
    },
    Look {
        schema_version: String,
        bundle_name: String,
        requester_session: String,
        target_session: String,
        captured_at: String,
        snapshot_lines: Vec<String>,
    },
    Error {
        error: RelayError,
    },
}

#[derive(Clone, Debug)]
struct ChatRequestContext {
    request_id: Option<String>,
    sender_session: String,
    message: String,
    targets: Vec<String>,
    broadcast: bool,
    delivery_mode: ChatDeliveryMode,
    quiet_window_ms: Option<u64>,
    quiescence_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct LookRequestContext {
    requester_session: String,
    target_session: String,
    lines: Option<usize>,
    bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
struct AsyncDeliveryTask {
    bundle: BundleConfiguration,
    sender: crate::configuration::BundleMember,
    all_target_sessions: Vec<String>,
    target_session: String,
    message: String,
    message_id: String,
    quiescence: QuiescenceOptions,
    batch_settings: PromptBatchSettings,
    tmux_socket: PathBuf,
}

/// Handles one relay socket request/response exchange on a connected stream.
pub fn serve_connection(
    stream: &mut UnixStream,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<(), io::Error> {
    let request = match read_request(stream) {
        Ok(value) => value,
        Err(source) => {
            let response = RelayResponse::Error {
                error: relay_error(
                    "validation_invalid_arguments",
                    "failed to parse relay request",
                    Some(json!({"cause": source.to_string()})),
                ),
            };
            write_response(stream, &response)?;
            return Ok(());
        }
    };

    let response = match handle_request(
        request,
        configuration_root,
        &bundle_paths.bundle_name,
        &bundle_paths.tmux_socket,
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    };
    write_response(stream, &response)
}

/// Executes one relay request for a configured bundle.
pub fn handle_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, &bundle)?;
    match request {
        RelayRequest::List { sender_session } => {
            handle_list(&bundle, &authorization, sender_session)
        }
        RelayRequest::Chat {
            request_id,
            sender_session,
            message,
            targets,
            broadcast,
            delivery_mode,
            quiet_window_ms,
            quiescence_timeout_ms,
        } => handle_chat(
            &bundle,
            &authorization,
            ChatRequestContext {
                request_id,
                sender_session,
                message,
                targets,
                broadcast,
                delivery_mode,
                quiet_window_ms,
                quiescence_timeout_ms,
            },
            tmux_socket,
        ),
        RelayRequest::Look {
            requester_session,
            target_session,
            lines,
            bundle_name: request_bundle_name,
        } => handle_look(
            &bundle,
            &authorization,
            LookRequestContext {
                requester_session,
                target_session,
                lines,
                bundle_name: request_bundle_name,
            },
            tmux_socket,
        ),
    }
}

/// Reconciles configured bundle sessions against tmux state.
///
/// # Errors
///
/// Returns structured validation/configuration errors when bundle loading
/// fails, and internal failures when tmux session operations fail.
pub fn reconcile_bundle(
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
pub fn shutdown_bundle_runtime(tmux_socket: &Path) -> Result<ShutdownReport, RelayError> {
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

fn handle_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    sender_session: Option<String>,
) -> Result<RelayResponse, RelayError> {
    let sender_session = sender_session.ok_or_else(|| {
        relay_error(
            "validation_unknown_sender",
            "sender_session is required for list authorization",
            None,
        )
    })?;
    let sender = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "sender_session is not in bundle configuration",
                Some(json!({"sender_session": sender_session})),
            )
        })?;
    authorize_list(bundle, authorization, sender.id.as_str())?;
    let recipients = bundle
        .members
        .iter()
        .filter(|member| member.id != sender.id)
        .map(|member| Recipient {
            session_name: member.id.clone(),
            display_name: member.name.clone(),
        })
        .collect::<Vec<_>>();

    let response = RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        recipients,
    };
    if let RelayResponse::List {
        bundle_name,
        recipients,
        ..
    } = &response
    {
        emit_inscription(
            "relay.list.response",
            &json!({
                "bundle_name": bundle_name,
                "sender_session": sender.id,
                "recipient_count": recipients.len(),
            }),
        );
    }
    Ok(response)
}

fn handle_chat(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: ChatRequestContext,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let ChatRequestContext {
        request_id,
        sender_session,
        message,
        targets,
        broadcast,
        delivery_mode,
        quiet_window_ms,
        quiescence_timeout_ms,
    } = request;

    if message.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if !broadcast && targets.is_empty() {
        return Err(relay_error(
            "validation_empty_targets",
            "targets must contain at least one session",
            None,
        ));
    }
    if broadcast && !targets.is_empty() {
        return Err(relay_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if matches!(quiescence_timeout_ms, Some(0)) {
        return Err(relay_error(
            "validation_invalid_quiescence_timeout",
            "quiescence timeout override must be greater than zero milliseconds",
            None,
        ));
    }

    let sender = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
        .cloned()
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "sender_session is not in bundle configuration",
                Some(json!({"sender_session": sender_session})),
            )
        })?;

    emit_inscription(
        "relay.chat.request",
        &json!({
            "bundle_name": bundle.bundle_name,
            "sender_session": sender.id,
            "broadcast": broadcast,
            "delivery_mode": delivery_mode,
            "target_count": targets.len(),
            "message_length": message.len(),
            "request_id": request_id.clone(),
        }),
    );

    let resolved_targets = if broadcast {
        bundle
            .members
            .iter()
            .filter(|member| member.id != sender.id)
            .map(|member| member.id.clone())
            .collect::<Vec<_>>()
    } else {
        resolve_explicit_targets(bundle, &targets)?
    };
    authorize_send(
        bundle,
        authorization,
        sender.id.as_str(),
        resolved_targets.as_slice(),
    )?;

    let all_target_sessions = resolved_targets.clone();
    let batch_settings = prompt_batch_settings();
    let (status, results) = match delivery_mode {
        ChatDeliveryMode::Sync => {
            let quiescence = QuiescenceOptions::for_sync(quiet_window_ms, quiescence_timeout_ms);
            let mut results = Vec::with_capacity(resolved_targets.len());
            for target_session in resolved_targets {
                let message_id = Uuid::new_v4().to_string();
                let task = AsyncDeliveryTask {
                    bundle: bundle.clone(),
                    sender: sender.clone(),
                    all_target_sessions: all_target_sessions.clone(),
                    target_session,
                    message: message.clone(),
                    message_id,
                    quiescence,
                    batch_settings,
                    tmux_socket: tmux_socket.to_path_buf(),
                };
                let result = deliver_one_target(&task)?;
                results.push(result);
            }
            (aggregate_chat_status(&results), results)
        }
        ChatDeliveryMode::Async => {
            let quiescence = QuiescenceOptions::for_async(quiet_window_ms, quiescence_timeout_ms);
            let mut results = Vec::with_capacity(resolved_targets.len());
            for target_session in resolved_targets {
                let message_id = Uuid::new_v4().to_string();
                let task = AsyncDeliveryTask {
                    bundle: bundle.clone(),
                    sender: sender.clone(),
                    all_target_sessions: all_target_sessions.clone(),
                    target_session: target_session.clone(),
                    message: message.clone(),
                    message_id: message_id.clone(),
                    quiescence,
                    batch_settings,
                    tmux_socket: tmux_socket.to_path_buf(),
                };
                enqueue_async_delivery(task)?;
                emit_inscription(
                    "relay.chat.async.queued",
                    &json!({
                        "bundle_name": bundle.bundle_name,
                        "sender_session": sender.id,
                        "target_session": target_session,
                        "message_id": message_id,
                    }),
                );
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Queued,
                    reason: None,
                });
            }
            (ChatStatus::Accepted, results)
        }
    };

    let response = RelayResponse::Chat {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        sender_session: sender.id.clone(),
        sender_display_name: sender.name.clone(),
        delivery_mode,
        status,
        results,
    };
    if let RelayResponse::Chat {
        bundle_name,
        sender_session,
        status,
        results,
        ..
    } = &response
    {
        let delivered_count = results
            .iter()
            .filter(|result| result.outcome == ChatOutcome::Delivered)
            .count();
        emit_inscription(
            "relay.chat.response",
            &json!({
            "bundle_name": bundle_name,
            "sender_session": sender_session,
            "delivery_mode": delivery_mode,
            "status": status,
            "result_count": results.len(),
            "delivered_count": delivered_count,
            }),
        );
    }
    Ok(response)
}

fn handle_look(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: LookRequestContext,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let LookRequestContext {
        requester_session,
        target_session,
        lines,
        bundle_name: request_bundle_name,
    } = request;
    if let Some(request_bundle_name) = request_bundle_name.as_deref()
        && request_bundle_name != bundle.bundle_name
    {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "look is limited to the associated bundle in MVP",
            Some(json!({
                "associated_bundle_name": bundle.bundle_name,
                "requested_bundle_name": request_bundle_name,
            })),
        ));
    }

    let requested_lines = lines.unwrap_or(DEFAULT_LOOK_LINES);
    if !(1..=MAX_LOOK_LINES).contains(&requested_lines) {
        return Err(relay_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": requested_lines,
                "min": 1,
                "max": MAX_LOOK_LINES,
            })),
        ));
    }

    let requester = bundle
        .members
        .iter()
        .find(|member| member.id == requester_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "requester_session is not in bundle configuration",
                Some(json!({"requester_session": requester_session})),
            )
        })?;
    let target = bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not in bundle configuration",
                Some(json!({"target_session": target_session})),
            )
        })?;
    authorize_look(
        bundle,
        authorization,
        requester.id.as_str(),
        target.id.as_str(),
    )?;

    let pane_target =
        resolve_active_pane_target(tmux_socket, target.id.as_str()).map_err(|reason| {
            relay_error(
                "internal_unexpected_failure",
                "failed to resolve active pane for look target",
                Some(json!({"target_session": target.id, "cause": reason})),
            )
        })?;
    let snapshot_lines =
        capture_pane_tail_lines(tmux_socket, pane_target.as_str(), requested_lines).map_err(
            |reason| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to capture look snapshot",
                    Some(json!({"target_session": target.id, "cause": reason})),
                )
            },
        )?;
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        requester_session: requester.id.clone(),
        target_session: target.id.clone(),
        captured_at,
        snapshot_lines,
    };
    if let RelayResponse::Look {
        bundle_name,
        requester_session,
        target_session,
        snapshot_lines,
        ..
    } = &response
    {
        emit_inscription(
            "relay.look.response",
            &json!({
                "bundle_name": bundle_name,
                "requester_session": requester_session,
                "target_session": target_session,
                "snapshot_line_count": snapshot_lines.len(),
                "lines_requested": requested_lines,
            }),
        );
    }
    Ok(response)
}

/// Waits for async delivery workers to stop after shutdown is requested.
///
/// Returns the number of workers still running when timeout is reached.
#[must_use]
pub fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    delivery::wait_for_async_delivery_shutdown(timeout)
}

fn resolve_explicit_targets(
    bundle: &BundleConfiguration,
    targets: &[String],
) -> Result<Vec<String>, RelayError> {
    let mut resolved = Vec::with_capacity(targets.len());
    let mut unknown_targets = Vec::new();

    for target in targets {
        let requested = target.trim();
        if requested.is_empty() {
            unknown_targets.push(target.clone());
            continue;
        }
        if let Some(member) = bundle.members.iter().find(|member| member.id == requested) {
            resolved.push(member.id.clone());
            continue;
        }

        let matched_by_name = bundle
            .members
            .iter()
            .filter(|member| member.name.as_deref() == Some(requested))
            .collect::<Vec<_>>();
        match matched_by_name.as_slice() {
            [] => unknown_targets.push(target.clone()),
            [member] => resolved.push(member.id.clone()),
            _ => {
                let matching_sessions = matched_by_name
                    .iter()
                    .map(|member| member.id.clone())
                    .collect::<Vec<_>>();
                return Err(relay_error(
                    "validation_ambiguous_recipient",
                    "target matches multiple configured session names",
                    Some(json!({
                        "target": target,
                        "matching_sessions": matching_sessions,
                    })),
                ));
            }
        }
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_recipient",
            "one or more targets are not in bundle configuration",
            Some(json!({"unknown_targets": unknown_targets})),
        ));
    }
    Ok(resolved)
}

fn reconcile_loaded_bundle(
    bundle: &BundleConfiguration,
    tmux_socket: &Path,
) -> Result<ReconciliationReport, RelayError> {
    let configured_sessions = bundle
        .members
        .iter()
        .filter(|member| member.start_command.is_some())
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();
    let mut missing = bundle
        .members
        .iter()
        .filter(|member| member.start_command.is_some())
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
    if let Some(start_command) = member.start_command.as_ref() {
        arguments.push(start_command.clone());
    }
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

fn read_request(stream: &UnixStream) -> Result<RelayRequest, io::Error> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request line is empty",
        ));
    }
    serde_json::from_str::<RelayRequest>(line.trim_end()).map_err(io::Error::other)
}

fn write_response(stream: &mut UnixStream, response: &RelayResponse) -> Result<(), io::Error> {
    let encoded = serde_json::to_string(response).map_err(io::Error::other)?;
    stream.write_all(encoded.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn map_config(error: ConfigurationError) -> RelayError {
    match error {
        ConfigurationError::UnknownBundle { bundle_name, path } => relay_error(
            "validation_unknown_bundle",
            "bundle is not configured",
            Some(json!({"bundle_name": bundle_name, "path": path})),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration is invalid",
            Some(json!({"path": path, "cause": message})),
        ),
        ConfigurationError::InvalidGroupName { path, group_name } => relay_error(
            "validation_invalid_group_name",
            "bundle configuration uses invalid group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => relay_error(
            "validation_reserved_group_name",
            "bundle configuration uses reserved group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::AmbiguousSender {
            working_directory,
            matches,
        } => relay_error(
            "validation_unknown_sender",
            "sender association is ambiguous",
            Some(json!({"working_directory": working_directory, "matches": matches})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration could not be loaded",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
    }
}

fn relay_error(code: &str, message: &str, details: Option<Value>) -> RelayError {
    RelayError {
        code: code.to_string(),
        message: message.to_string(),
        details,
    }
}

/// Sends one request to relay socket and returns the parsed response.
pub fn request_relay(
    socket_path: &Path,
    request: &RelayRequest,
) -> Result<RelayResponse, io::Error> {
    let mut stream = UnixStream::connect(socket_path)?;
    let request_text = serde_json::to_string(request).map_err(io::Error::other)?;
    stream.write_all(request_text.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "relay returned empty response",
        ));
    }
    serde_json::from_str::<RelayResponse>(line.trim_end()).map_err(io::Error::other)
}
