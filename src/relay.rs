//! Relay IPC contract and message-routing implementation.

use std::{
    collections::HashSet,
    ffi::OsStr,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{
        BundleConfiguration, ConfigurationError, PromptReadinessTemplate, load_bundle_configuration,
    },
    envelope::{
        AddressIdentity, ENVELOPE_SCHEMA_VERSION, EnvelopeRenderInput, ManifestPreamble,
        PromptBatchSettings, batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::paths::BundleRuntimePaths,
};

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const DEFAULT_QUIET_WINDOW_MS: u64 = 750;
const DEFAULT_DELIVERY_TIMEOUT_MS: u64 = 30_000;
const OWNERSHIP_OPTION_NAME: &str = "@tmuxmux_owned";
const OWNERSHIP_OPTION_VALUE: &str = "1";
const CREATE_MAX_ATTEMPTS: usize = 4;
const CREATE_RETRY_BASE_DELAY_MS: u64 = 35;
const CREATE_RETRY_JITTER_MS: u64 = 35;
const MAX_PROMPT_TOKENS_ENVVAR: &str = "TMUXMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "TMUXMUX_TOKENIZER_PROFILE";
const DEFAULT_PROMPT_INSPECT_LINES: usize = 3;
const MAX_PROMPT_INSPECT_LINES: usize = 40;

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

/// Aggregate delivery status for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Success,
    Partial,
    Failure,
}

/// Per-target delivery outcome for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatOutcome {
    Delivered,
    Timeout,
    Failed,
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
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        delivery_timeout_ms: Option<u64>,
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
        status: ChatStatus,
        results: Vec<ChatResult>,
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
    quiet_window_ms: Option<u64>,
    delivery_timeout_ms: Option<u64>,
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
    match request {
        RelayRequest::List { sender_session } => Ok(handle_list(&bundle, sender_session)),
        RelayRequest::Chat {
            request_id,
            sender_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            delivery_timeout_ms,
        } => handle_chat(
            &bundle,
            ChatRequestContext {
                request_id,
                sender_session,
                message,
                targets,
                broadcast,
                quiet_window_ms,
                delivery_timeout_ms,
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
    reconcile_loaded_bundle(&bundle, tmux_socket)
}

fn handle_list(bundle: &BundleConfiguration, sender_session: Option<String>) -> RelayResponse {
    let recipients = bundle
        .members
        .iter()
        .filter(|member| Some(member.id.as_str()) != sender_session.as_deref())
        .map(|member| Recipient {
            session_name: member.id.clone(),
            display_name: member.name.clone(),
        })
        .collect::<Vec<_>>();

    RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        recipients,
    }
}

fn handle_chat(
    bundle: &BundleConfiguration,
    request: ChatRequestContext,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let ChatRequestContext {
        request_id,
        sender_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        delivery_timeout_ms,
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

    let mut results = Vec::with_capacity(resolved_targets.len());
    let all_target_sessions = resolved_targets.clone();
    let batch_settings = prompt_batch_settings();
    let quiescence = QuiescenceOptions::new(quiet_window_ms, delivery_timeout_ms);
    for target_session in resolved_targets {
        let message_id = Uuid::new_v4().to_string();
        let created_at = time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        let target_member = bundle
            .members
            .iter()
            .find(|member| member.id == target_session)
            .ok_or_else(|| {
                relay_error(
                    "internal_unexpected_failure",
                    "resolved target member is missing from bundle configuration",
                    Some(json!({"target_session": target_session})),
                )
            })?;
        let cc_members = all_target_sessions
            .iter()
            .filter(|candidate| **candidate != target_session)
            .filter_map(|session_name| {
                bundle
                    .members
                    .iter()
                    .find(|member| member.id == *session_name)
            })
            .cloned()
            .collect::<Vec<_>>();
        match wait_for_quiescent_pane(
            tmux_socket,
            &target_session,
            quiescence,
            target_member.prompt_readiness.as_ref(),
        ) {
            Ok(pane_target) => {
                let envelope = render_envelope(&EnvelopeRenderInput {
                    manifest: ManifestPreamble {
                        schema_version: SCHEMA_VERSION.to_string(),
                        message_id: message_id.clone(),
                        bundle_name: bundle.bundle_name.clone(),
                        sender_session: sender.id.clone(),
                        target_sessions: vec![target_session.clone()],
                        cc_sessions: if cc_members.is_empty() {
                            None
                        } else {
                            Some(
                                cc_members
                                    .iter()
                                    .map(|member| member.id.clone())
                                    .collect::<Vec<_>>(),
                            )
                        },
                        created_at,
                    },
                    from: AddressIdentity {
                        session_name: sender.id.clone(),
                        display_name: sender.name.clone(),
                    },
                    to: vec![AddressIdentity {
                        session_name: target_member.id.clone(),
                        display_name: target_member.name.clone(),
                    }],
                    cc: cc_members
                        .iter()
                        .map(|member| AddressIdentity {
                            session_name: member.id.clone(),
                            display_name: member.name.clone(),
                        })
                        .collect::<Vec<_>>(),
                    subject: None,
                    body: message.clone(),
                });
                let prompt_batches = batch_envelopes(&[envelope], batch_settings);
                let mut failed_reason = None::<String>;
                for prompt in prompt_batches {
                    if let Err(reason) = inject_prompt(tmux_socket, &pane_target, &prompt) {
                        failed_reason = Some(reason);
                        break;
                    }
                }
                match failed_reason {
                    None => {
                        results.push(ChatResult {
                            target_session,
                            message_id,
                            outcome: ChatOutcome::Delivered,
                            reason: None,
                        });
                    }
                    Some(reason) => {
                        results.push(ChatResult {
                            target_session,
                            message_id,
                            outcome: ChatOutcome::Failed,
                            reason: Some(reason),
                        });
                    }
                }
            }
            Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
            }) => {
                let reason = if readiness_mismatch {
                    format!(
                        "prompt readiness did not match before timeout after {}ms",
                        timeout.as_millis()
                    )
                } else {
                    format!("quiescence wait timed out after {}ms", timeout.as_millis())
                };
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Timeout,
                    reason: Some(reason),
                });
            }
            Err(DeliveryWaitError::Failed { reason }) => {
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Failed,
                    reason: Some(reason),
                });
            }
        }
    }

    Ok(RelayResponse::Chat {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        sender_session: sender.id.clone(),
        sender_display_name: sender.name.clone(),
        status: aggregate_chat_status(&results),
        results,
    })
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
        .map(|member| member.id.clone())
        .collect::<HashSet<_>>();
    let mut missing = bundle
        .members
        .iter()
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

    cleanup_tmux_server_when_unowned(tmux_socket)?;
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
        || lowered.contains("no server running")
        || lowered.contains("no such file or directory")
        || lowered.contains("error connecting")
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
            "failed to prune tmuxmux-owned session",
            Some(json!({"session_name": session_name, "cause": reason})),
        )
    })
}

fn list_owned_sessions(tmux_socket: &Path) -> Result<Vec<String>, RelayError> {
    let output = match run_tmux_command_capture(
        tmux_socket,
        &["list-sessions", "-F", "#{session_name}\t#{@tmuxmux_owned}"],
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

fn cleanup_tmux_server_when_unowned(tmux_socket: &Path) -> Result<(), RelayError> {
    if !list_owned_sessions(tmux_socket)?.is_empty() {
        return Ok(());
    }
    if !list_all_sessions(tmux_socket)?.is_empty() {
        return Ok(());
    }
    let output = run_tmux_command_capture(tmux_socket, &["kill-server"]).map_err(|reason| {
        relay_error(
            "internal_unexpected_failure",
            "failed to clean up tmux socket",
            Some(json!({"cause": reason})),
        )
    })?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if reason.to_ascii_lowercase().contains("no server running") {
        return Ok(());
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

fn aggregate_chat_status(results: &[ChatResult]) -> ChatStatus {
    let delivered = results
        .iter()
        .filter(|result| result.outcome == ChatOutcome::Delivered)
        .count();
    if delivered == results.len() {
        return ChatStatus::Success;
    }
    if delivered > 0 {
        return ChatStatus::Partial;
    }
    ChatStatus::Failure
}

fn prompt_batch_settings() -> PromptBatchSettings {
    let max_prompt_tokens = std::env::var(MAX_PROMPT_TOKENS_ENVVAR)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(PromptBatchSettings::default().max_prompt_tokens);
    let tokenizer_profile = std::env::var(TOKENIZER_PROFILE_ENVVAR)
        .ok()
        .as_deref()
        .and_then(parse_tokenizer_profile)
        .unwrap_or_default();
    PromptBatchSettings {
        max_prompt_tokens,
        tokenizer_profile,
    }
}

#[derive(Clone, Copy, Debug)]
struct QuiescenceOptions {
    quiet_window: Duration,
    delivery_timeout: Duration,
}

impl Default for QuiescenceOptions {
    fn default() -> Self {
        Self {
            quiet_window: Duration::from_millis(DEFAULT_QUIET_WINDOW_MS),
            delivery_timeout: Duration::from_millis(DEFAULT_DELIVERY_TIMEOUT_MS),
        }
    }
}

impl QuiescenceOptions {
    fn new(quiet_window_ms: Option<u64>, delivery_timeout_ms: Option<u64>) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_QUIET_WINDOW_MS),
            ),
            delivery_timeout: Duration::from_millis(
                delivery_timeout_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_DELIVERY_TIMEOUT_MS),
            ),
        }
    }
}

#[derive(Debug)]
enum DeliveryWaitError {
    Timeout {
        timeout: Duration,
        readiness_mismatch: bool,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    options: QuiescenceOptions,
    prompt_readiness: Option<&PromptReadinessTemplate>,
) -> Result<String, DeliveryWaitError> {
    let readiness = build_prompt_readiness_matcher(prompt_readiness)
        .map_err(|reason| DeliveryWaitError::Failed { reason })?;
    let deadline = Instant::now() + options.delivery_timeout;
    let mut readiness_mismatch = false;
    loop {
        let pane_before = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_before = capture_pane_snapshot(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(options.quiet_window);

        let pane_after = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_after = capture_pane_snapshot(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        if pane_before == pane_after && snapshot_before == snapshot_after {
            match prompt_readiness_matches(
                tmux_socket,
                pane_after.as_str(),
                snapshot_after.as_str(),
                readiness.as_ref(),
            ) {
                Ok(true) => return Ok(pane_after),
                Ok(false) => readiness_mismatch = true,
                Err(reason) => return Err(DeliveryWaitError::Failed { reason }),
            }
        }

        if Instant::now() >= deadline {
            return Err(DeliveryWaitError::Timeout {
                timeout: options.delivery_timeout,
                readiness_mismatch,
            });
        }
    }
}

fn build_prompt_readiness_matcher(
    template: Option<&PromptReadinessTemplate>,
) -> Result<Option<PromptReadinessMatcher>, String> {
    let Some(template) = template else {
        return Ok(None);
    };

    let prompt_regex = Regex::new(template.prompt_regex.as_str())
        .map_err(|source| format!("invalid prompt_readiness.prompt_regex: {source}"))?;
    let inspect_lines = template
        .inspect_lines
        .unwrap_or(DEFAULT_PROMPT_INSPECT_LINES)
        .clamp(1, MAX_PROMPT_INSPECT_LINES);

    Ok(Some(PromptReadinessMatcher {
        prompt_regex,
        inspect_lines,
        input_idle_cursor_column: template.input_idle_cursor_column,
    }))
}

fn prompt_readiness_matches(
    tmux_socket: &Path,
    pane_target: &str,
    snapshot: &str,
    matcher: Option<&PromptReadinessMatcher>,
) -> Result<bool, String> {
    let Some(matcher) = matcher else {
        return Ok(true);
    };

    let inspected = snapshot
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(matcher.inspect_lines)
        .collect::<Vec<_>>();
    if inspected.is_empty() {
        return Ok(false);
    }
    let mut ordered = inspected;
    ordered.reverse();
    let block = ordered.join("\n");
    if !matcher.prompt_regex.is_match(block.as_str()) {
        return Ok(false);
    }

    let Some(expected_cursor_column) = matcher.input_idle_cursor_column else {
        return Ok(true);
    };
    let cursor_column = resolve_cursor_column(tmux_socket, pane_target)?;
    Ok(cursor_column == expected_cursor_column)
}

fn resolve_active_pane_target(tmux_socket: &Path, target_session: &str) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["display-message", "-p", "-t", target_session, "#{pane_id}"],
    )?;
    let pane_target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pane_target.is_empty() {
        return Err(format!(
            "tmux did not return an active pane for session {target_session}"
        ));
    }
    Ok(pane_target)
}

fn capture_pane_snapshot(tmux_socket: &Path, pane_target: &str) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["capture-pane", "-p", "-t", pane_target, "-S", "-200"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn resolve_cursor_column(tmux_socket: &Path, pane_target: &str) -> Result<usize, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["display-message", "-p", "-t", pane_target, "#{cursor_x}"],
    )?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    value
        .parse::<usize>()
        .map_err(|source| format!("failed to parse tmux cursor_x '{value}': {source}"))
}

fn inject_prompt(tmux_socket: &Path, pane_target: &str, prompt: &str) -> Result<(), String> {
    run_tmux_command(
        tmux_socket,
        &["send-keys", "-t", pane_target, "--", prompt, "Enter"],
    )?;
    Ok(())
}

fn run_tmux_command(
    tmux_socket: &Path,
    command_arguments: &[impl AsRef<OsStr>],
) -> Result<std::process::Output, String> {
    let output = run_tmux_command_capture(tmux_socket, command_arguments)?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let command_name = command_arguments
        .first()
        .map(|argument| argument.as_ref().to_string_lossy().to_string())
        .unwrap_or_else(|| "tmux".to_string());
    if stderr.is_empty() {
        return Err(format!("tmux {command_name} failed"));
    }
    Err(stderr)
}

fn run_tmux_command_capture(
    tmux_socket: &Path,
    command_arguments: &[impl AsRef<OsStr>],
) -> Result<std::process::Output, String> {
    let mut command = Command::new(tmux_program());
    command.arg("-S").arg(tmux_socket).args(command_arguments);
    command.output().map_err(|source| source.to_string())
}

fn tmux_program() -> String {
    std::env::var("TMUXMUX_TMUX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "tmux".to_string())
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
