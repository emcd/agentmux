use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::{AcpTargetConfiguration, PromptReadinessTemplate, TargetConfiguration},
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::stream::{
    RelayClientClass, RelayStreamEvent, StreamEventSendOutcome, resolve_registered_client_class,
    send_event_to_registered_ui,
};
use super::tmux::{
    capture_pane_snapshot, emit_delivery_diagnostic, inject_prompt, operator_interaction_active,
    resolve_active_pane_target, resolve_cursor_column, resolve_window_activity_marker,
    sanitize_diagnostic_text,
};
use super::{AsyncDeliveryTask, ChatOutcome, ChatResult, ChatStatus, RelayError, SCHEMA_VERSION};

const DEFAULT_QUIET_WINDOW_MS: u64 = 750;
const DEFAULT_QUIESCENCE_TIMEOUT_MS: u64 = 30_000;
const MAX_PROMPT_TOKENS_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";
const DEFAULT_PROMPT_INSPECT_LINES: usize = 3;
const MAX_PROMPT_INSPECT_LINES: usize = 40;
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";
const ACP_PROTOCOL_VERSION: u32 = 1;
const UI_RECONNECT_POLL_INTERVAL_MS: u64 = 100;
const ACP_SESSION_STATE_SCHEMA_VERSION: u32 = 1;
const ACP_SESSIONS_DIRECTORY: &str = "sessions";
const ACP_SESSION_STATE_FILE: &str = "state.json";
const ACP_LOOK_SNAPSHOT_MAX_LINES: usize = 1000;
const ACP_REASON_CODE_TURN_TIMEOUT: &str = "acp_turn_timeout";
const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

#[derive(Debug)]
enum AcpRequestError {
    Failed(String),
    Timeout(Duration),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedAcpSessionState {
    schema_version: u32,
    acp_session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    snapshot_lines: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum AcpLifecycleSelection {
    NewSession,
    LoadSession,
}

#[derive(Clone, Debug)]
struct AcpCapabilities {
    load_session: bool,
    prompt_session: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct QuiescenceOptions {
    quiet_window: Duration,
    quiescence_timeout: Option<Duration>,
}

impl Default for QuiescenceOptions {
    fn default() -> Self {
        Self {
            quiet_window: Duration::from_millis(DEFAULT_QUIET_WINDOW_MS),
            quiescence_timeout: Some(Duration::from_millis(DEFAULT_QUIESCENCE_TIMEOUT_MS)),
        }
    }
}

impl QuiescenceOptions {
    pub(super) fn for_sync(
        quiet_window_ms: Option<u64>,
        quiescence_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_QUIET_WINDOW_MS),
            ),
            quiescence_timeout: Some(Duration::from_millis(
                quiescence_timeout_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_QUIESCENCE_TIMEOUT_MS),
            )),
        }
    }

    pub(super) fn for_async(
        quiet_window_ms: Option<u64>,
        quiescence_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_QUIET_WINDOW_MS),
            ),
            quiescence_timeout: quiescence_timeout_ms
                .filter(|value| *value > 0)
                .map(Duration::from_millis),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct AsyncWorkerKey {
    tmux_socket: PathBuf,
    bundle_name: String,
    target_session: String,
}

#[derive(Default)]
struct AsyncDeliveryRegistry {
    workers: Mutex<HashMap<AsyncWorkerKey, mpsc::Sender<AsyncDeliveryTask>>>,
}

static ASYNC_DELIVERY_REGISTRY: OnceLock<AsyncDeliveryRegistry> = OnceLock::new();
static ACP_SESSION_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug)]
enum DeliveryWaitError {
    Timeout {
        timeout: Duration,
        readiness_mismatch: bool,
        mismatch_reason: Option<String>,
    },
    Failed {
        reason: String,
    },
    Shutdown,
}

#[derive(Debug)]
struct PromptReadinessMatcher {
    prompt_regex: Regex,
    inspect_lines: usize,
    input_idle_cursor_column: Option<usize>,
}

#[derive(Debug, Default)]
struct PromptReadinessEvaluation {
    ready: bool,
    mismatch_reason: Option<String>,
    inspected_block: Option<String>,
    regex_matched: Option<bool>,
    expected_cursor_column: Option<usize>,
    observed_cursor_column: Option<usize>,
}

pub(super) fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    if !shutdown_requested() {
        return 0;
    }
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = async_worker_count();
        if remaining == 0 || Instant::now() >= deadline {
            return remaining;
        }
        thread::sleep(Duration::from_millis(ASYNC_SHUTDOWN_WAIT_POLL_MS));
    }
}

pub(super) fn enqueue_async_delivery(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let key = AsyncWorkerKey {
        tmux_socket: task.tmux_socket.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        target_session: task.target_session.clone(),
    };
    let registry = async_delivery_registry();
    let mut workers = registry.workers.lock().map_err(|_| {
        super::relay_error(
            "internal_unexpected_failure",
            "failed to lock async delivery registry",
            None,
        )
    })?;

    if let Some(sender) = workers.get(&key) {
        if sender.send(task.clone()).is_ok() {
            return Ok(());
        }
        workers.remove(&key);
    }

    let (sender, receiver) = mpsc::channel::<AsyncDeliveryTask>();
    sender.send(task).map_err(|source| {
        super::relay_error(
            "internal_unexpected_failure",
            "failed to enqueue async delivery task",
            Some(json!({"cause": source.to_string()})),
        )
    })?;
    spawn_async_delivery_worker(key.clone(), receiver);
    workers.insert(key, sender);
    Ok(())
}

pub(super) fn deliver_one_target(task: &AsyncDeliveryTask) -> Result<ChatResult, RelayError> {
    let bundle = &task.bundle;
    let sender = &task.sender;
    let all_target_sessions = &task.all_target_sessions;
    let target_session = task.target_session.clone();
    let message = task.message.as_str();
    let message_id = task.message_id.clone();
    let tmux_socket = task.tmux_socket.as_path();
    let quiescence = task.quiescence;
    let batch_settings = task.batch_settings;
    let created_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let target_member = bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
        .ok_or_else(|| {
            super::relay_error(
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

    let manifest = ManifestPreamble {
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
    };
    emit_inscription(
        "relay.chat.envelope.metadata",
        &json!({
            "schema_version": manifest.schema_version,
            "message_id": manifest.message_id,
            "bundle_name": manifest.bundle_name,
            "sender_session": manifest.sender_session,
            "target_sessions": manifest.target_sessions,
            "cc_sessions": manifest.cc_sessions,
            "created_at": manifest.created_at,
        }),
    );
    let envelope = render_envelope(&EnvelopeRenderInput {
        manifest,
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
        body: message.to_string(),
    });
    let prompt_batches = batch_envelopes(&[envelope], batch_settings);
    let resolved_client_class =
        resolve_registered_client_class(bundle.bundle_name.as_str(), target_session.as_str())
            .map_err(|source| {
                super::relay_error(
                    "internal_unexpected_failure",
                    "failed to resolve relay stream endpoint class",
                    Some(json!({
                        "bundle_name": bundle.bundle_name,
                        "target_session": target_session,
                        "cause": source.to_string(),
                    })),
                )
            })?;
    if matches!(resolved_client_class, Some(RelayClientClass::Ui)) {
        return Ok(deliver_one_target_ui(
            task,
            sender,
            &cc_members,
            target_session,
            message_id,
            message,
        ));
    }

    match &target_member.target {
        TargetConfiguration::Acp(acp) => Ok(deliver_one_target_acp(
            task,
            target_member,
            acp,
            prompt_batches,
            target_session,
            message_id,
        )),
        TargetConfiguration::Tmux(tmux_target) => match wait_for_quiescent_pane(
            tmux_socket,
            &target_session,
            quiescence,
            tmux_target.prompt_readiness.as_ref(),
        ) {
            Ok(pane_target) => {
                let mut failed_reason = None::<String>;
                for prompt in prompt_batches {
                    if let Err(reason) = inject_prompt(tmux_socket, &pane_target, &prompt) {
                        failed_reason = Some(reason);
                        break;
                    }
                }
                match failed_reason {
                    None => Ok(ChatResult {
                        target_session,
                        message_id,
                        outcome: ChatOutcome::Delivered,
                        reason_code: None,
                        reason: None,
                        details: None,
                    }),
                    Some(reason) => Ok(ChatResult {
                        target_session,
                        message_id,
                        outcome: ChatOutcome::Failed,
                        reason_code: None,
                        reason: Some(reason),
                        details: None,
                    }),
                }
            }
            Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
                mismatch_reason,
            }) => {
                let reason = if readiness_mismatch {
                    let detail = mismatch_reason
                        .map(|value| format!(": {value}"))
                        .unwrap_or_default();
                    format!(
                        "prompt readiness did not match before timeout after {}ms{}",
                        timeout.as_millis(),
                        detail
                    )
                } else {
                    format!("quiescence wait timed out after {}ms", timeout.as_millis())
                };
                Ok(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Timeout,
                    reason_code: None,
                    reason: Some(reason),
                    details: None,
                })
            }
            Err(DeliveryWaitError::Failed { reason }) => Ok(ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::Failed,
                reason_code: None,
                reason: Some(reason),
                details: None,
            }),
            Err(DeliveryWaitError::Shutdown) => Ok(ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            }),
        },
    }
}

fn deliver_one_target_ui(
    task: &AsyncDeliveryTask,
    sender: &crate::configuration::BundleMember,
    cc_members: &[crate::configuration::BundleMember],
    target_session: String,
    message_id: String,
    message: &str,
) -> ChatResult {
    let bundle_name = task.bundle.bundle_name.as_str();
    let timeout = task.quiescence.quiescence_timeout;
    let start = Instant::now();
    loop {
        if shutdown_requested() {
            return ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::DroppedOnShutdown,
                reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
                reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
                details: None,
            };
        }

        let incoming_event = RelayStreamEvent {
            event_type: "incoming_message".to_string(),
            bundle_name: bundle_name.to_string(),
            target_session: target_session.clone(),
            created_at: timestamp_rfc3339(),
            payload: json!({
                "message_id": message_id.clone(),
                "sender_session": sender.id.as_str(),
                "body": message,
                "cc_sessions": if cc_members.is_empty() {
                    Value::Null
                } else {
                    json!(cc_members.iter().map(|member| member.id.clone()).collect::<Vec<_>>())
                },
            }),
        };
        match send_event_to_registered_ui(bundle_name, target_session.as_str(), &incoming_event) {
            Ok(StreamEventSendOutcome::Delivered) => {
                let outcome_event = RelayStreamEvent {
                    event_type: "delivery_outcome".to_string(),
                    bundle_name: bundle_name.to_string(),
                    target_session: target_session.clone(),
                    created_at: timestamp_rfc3339(),
                    payload: json!({
                        "message_id": message_id.clone(),
                        "outcome": "success",
                    }),
                };
                let _ = send_event_to_registered_ui(
                    bundle_name,
                    target_session.as_str(),
                    &outcome_event,
                );
                return ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Delivered,
                    reason_code: None,
                    reason: None,
                    details: None,
                };
            }
            Ok(StreamEventSendOutcome::NoUiEndpoint) | Ok(StreamEventSendOutcome::Disconnected) => {
            }
            Err(source) => {
                return ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Failed,
                    reason_code: None,
                    reason: Some(format!("failed to emit relay stream event: {}", source)),
                    details: None,
                };
            }
        }
        if timeout.is_some_and(|value| start.elapsed() >= value) {
            return ChatResult {
                target_session,
                message_id,
                outcome: ChatOutcome::Timeout,
                reason_code: None,
                reason: Some(format!(
                    "ui relay stream was disconnected for {}ms",
                    start.elapsed().as_millis()
                )),
                details: None,
            };
        }
        thread::sleep(Duration::from_millis(UI_RECONNECT_POLL_INTERVAL_MS));
    }
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn delivered_result(target_session: String, message_id: String) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

fn failed_result(
    target_session: String,
    message_id: String,
    reason: impl Into<String>,
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Failed,
        reason_code: None,
        reason: Some(reason.into()),
        details: None,
    }
}

fn failed_result_with_code(
    target_session: String,
    message_id: String,
    reason_code: &str,
    reason: impl Into<String>,
    details: Option<Value>,
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.into()),
        details,
    }
}

fn timeout_result(
    target_session: String,
    message_id: String,
    reason_code: Option<&str>,
    reason: impl Into<String>,
) -> ChatResult {
    ChatResult {
        target_session,
        message_id,
        outcome: ChatOutcome::Timeout,
        reason_code: reason_code.map(ToString::to_string),
        reason: Some(reason.into()),
        details: None,
    }
}

fn deliver_one_target_acp(
    task: &AsyncDeliveryTask,
    target_member: &crate::configuration::BundleMember,
    acp: &AcpTargetConfiguration,
    prompt_batches: Vec<String>,
    target_session: String,
    message_id: String,
) -> ChatResult {
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return failed_result(
            target_session,
            message_id,
            "ACP target is missing working directory",
        );
    };

    let mut client = match acp.channel {
        crate::configuration::AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return failed_result(
                    target_session,
                    message_id,
                    "ACP stdio target requires command",
                );
            };
            match AcpStdioClient::spawn(command, working_directory) {
                Ok(client) => client,
                Err(reason) => return failed_result(target_session, message_id, reason),
            }
        }
        crate::configuration::AcpChannel::Http => {
            return failed_result(
                target_session,
                message_id,
                "ACP http transport is not implemented",
            );
        }
    };

    let initialize_result = match client.initialize() {
        Ok(value) => value,
        Err(reason) => {
            return failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_INITIALIZE_FAILED,
                "ACP initialize failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            );
        }
    };

    let capabilities = AcpCapabilities {
        load_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt_session: initialize_result
            .get("agentCapabilities")
            .and_then(|value| value.get("promptSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };

    let runtime_socket_path = task.tmux_socket.as_path();

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        match load_persisted_acp_session_id(runtime_socket_path, target_member.id.as_str()) {
            Ok(value) => value,
            Err(reason) => {
                return failed_result(
                    target_session,
                    message_id,
                    format!("failed to load persisted ACP session id: {reason}"),
                );
            }
        }
    };

    let (lifecycle, lifecycle_session_id) =
        if let Some(configured) = target_member.coder_session_id.as_deref() {
            (AcpLifecycleSelection::LoadSession, configured.to_string())
        } else if let Some(persisted) = persisted_session_id {
            (AcpLifecycleSelection::LoadSession, persisted)
        } else {
            (AcpLifecycleSelection::NewSession, String::new())
        };

    let session_id = match lifecycle {
        AcpLifecycleSelection::LoadSession => {
            if !capabilities.load_session {
                return failed_result_with_code(
                    target_session,
                    message_id,
                    ACP_ERROR_CODE_MISSING_CAPABILITY,
                    "ACP agent does not advertise required load capability",
                    Some(json!({
                        "target_session": target_member.id,
                        "required_capability": "session/load",
                        "reason": "agentCapabilities.loadSession is false or missing",
                    })),
                );
            }
            if let Err(reason) =
                client.load_session(lifecycle_session_id.as_str(), working_directory)
            {
                return failed_result(
                    target_session,
                    message_id,
                    format!("ACP session/load failed: {reason}"),
                );
            }
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => match client.new_session(working_directory) {
            Ok(value) => value,
            Err(reason) => {
                return failed_result(
                    target_session,
                    message_id,
                    format!("ACP session/new failed: {reason}"),
                );
            }
        },
    };

    if let Err(reason) = persist_acp_session_id(
        runtime_socket_path,
        target_member.id.as_str(),
        session_id.as_str(),
    ) {
        return failed_result(
            target_session,
            message_id,
            format!("failed to persist ACP session id: {reason}"),
        );
    }

    if !capabilities.prompt_session {
        return failed_result_with_code(
            target_session,
            message_id,
            ACP_ERROR_CODE_MISSING_CAPABILITY,
            "ACP agent does not advertise required prompt capability",
            Some(json!({
                "target_session": target_member.id,
                "required_capability": "session/prompt",
                "reason": "agentCapabilities.promptSession is false or missing",
            })),
        );
    }

    let turn_timeout = task.quiescence.quiescence_timeout;
    for prompt in prompt_batches {
        let prompt_result = client.prompt(session_id.as_str(), prompt.as_str(), turn_timeout);
        let prompt_snapshot_lines = client.take_snapshot_lines();
        if let Err(reason) = persist_acp_snapshot_lines(
            runtime_socket_path,
            target_member.id.as_str(),
            session_id.as_str(),
            prompt_snapshot_lines.as_slice(),
        ) {
            return failed_result(
                target_session,
                message_id,
                format!("failed to persist ACP look snapshot state: {reason}"),
            );
        }
        match prompt_result {
            Ok(stop_reason) => match stop_reason.as_str() {
                "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => {}
                "cancelled" => {
                    return failed_result_with_code(
                        target_session,
                        message_id,
                        ACP_REASON_CODE_STOP_CANCELLED,
                        "ACP turn completed with stopReason=cancelled",
                        None,
                    );
                }
                _ => {
                    return failed_result(
                        target_session,
                        message_id,
                        format!("ACP returned unsupported stopReason '{stop_reason}'"),
                    );
                }
            },
            Err(AcpRequestError::Timeout(timeout)) => {
                return timeout_result(
                    target_session,
                    message_id,
                    Some(ACP_REASON_CODE_TURN_TIMEOUT),
                    format!(
                        "ACP session/prompt timed out after {}ms",
                        timeout.as_millis()
                    ),
                );
            }
            Err(AcpRequestError::Failed(reason)) => {
                return failed_result(
                    target_session,
                    message_id,
                    format!("ACP session/prompt failed: {reason}"),
                );
            }
        }
    }

    delivered_result(target_session, message_id)
}

fn resolve_acp_session_state_path(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<PathBuf, String> {
    // Runtime state is anchored at the bundle runtime directory that contains
    // the socket path (`<state-root>/bundles/<bundle>`).
    let Some(runtime_directory) = runtime_socket_path.parent() else {
        return Err("runtime socket path has no parent runtime directory".to_string());
    };
    Ok(runtime_directory
        .join(ACP_SESSIONS_DIRECTORY)
        .join(target_session)
        .join(ACP_SESSION_STATE_FILE))
}

fn acp_session_state_lock() -> &'static Mutex<()> {
    ACP_SESSION_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

fn load_persisted_acp_session_id(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<Option<String>, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    Ok(state.map(|value| value.acp_session_id))
}

pub(super) fn load_acp_snapshot_lines_for_look(
    runtime_socket_path: &Path,
    target_session: &str,
    requested_lines: usize,
) -> Result<Vec<String>, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    let Some(state) = state else {
        return Ok(Vec::new());
    };
    let count = state.snapshot_lines.len();
    if requested_lines >= count {
        return Ok(state.snapshot_lines);
    }
    Ok(state.snapshot_lines[count - requested_lines..].to_vec())
}

fn persist_acp_session_id(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: &str,
) -> Result<(), String> {
    persist_acp_snapshot_lines(runtime_socket_path, target_session, session_id, &[])
}

fn persist_acp_snapshot_lines(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: &str,
    snapshot_lines: &[String],
) -> Result<(), String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let mut state =
        load_persisted_acp_session_state(path.as_path())?.unwrap_or(PersistedAcpSessionState {
            schema_version: ACP_SESSION_STATE_SCHEMA_VERSION,
            acp_session_id: session_id.to_string(),
            snapshot_lines: Vec::new(),
        });
    state.schema_version = ACP_SESSION_STATE_SCHEMA_VERSION;
    state.acp_session_id = session_id.to_string();
    append_snapshot_lines(
        &mut state.snapshot_lines,
        snapshot_lines,
        ACP_LOOK_SNAPSHOT_MAX_LINES,
    );
    store_persisted_acp_session_state(path.as_path(), &state)
}

fn append_snapshot_lines(storage: &mut Vec<String>, appended: &[String], max_lines: usize) {
    storage.extend(appended.iter().cloned());
    if storage.len() > max_lines {
        let overflow = storage.len() - max_lines;
        storage.drain(0..overflow);
    }
}

fn load_persisted_acp_session_state(
    path: &Path,
) -> Result<Option<PersistedAcpSessionState>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|source| format!("read ACP session state {} failed: {source}", path.display()))?;
    let state =
        serde_json::from_str::<PersistedAcpSessionState>(raw.as_str()).map_err(|source| {
            format!(
                "parse ACP session state {} failed: {source}",
                path.display()
            )
        })?;
    if state.schema_version != ACP_SESSION_STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported ACP session state schema_version '{}' in {}",
            state.schema_version,
            path.display()
        ));
    }
    if state.acp_session_id.trim().is_empty() {
        return Err(format!(
            "invalid ACP session state {}: acp_session_id must be non-empty",
            path.display()
        ));
    }
    Ok(Some(state))
}

fn store_persisted_acp_session_state(
    path: &Path,
    state: &PersistedAcpSessionState,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            format!(
                "create ACP session state directory {} failed: {source}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_string_pretty(state).map_err(|source| {
        format!(
            "encode ACP session state {} failed: {source}",
            path.display()
        )
    })?;
    fs::write(path, encoded).map_err(|source| {
        format!(
            "write ACP session state {} failed: {source}",
            path.display()
        )
    })
}

struct AcpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    read_buffer: Vec<u8>,
    next_id: u64,
    snapshot_line_buffer: Vec<String>,
}

impl AcpStdioClient {
    fn spawn(command_template: &str, working_directory: &Path) -> Result<Self, String> {
        let mut command = Command::new("sh");
        command
            .arg("-lc")
            .arg(command_template)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|source| format!("spawn ACP stdio command failed: {source}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ACP stdio child stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ACP stdio child stdout unavailable".to_string())?;
        set_nonblocking(stdout.as_raw_fd(), true)?;
        Ok(Self {
            child,
            stdin,
            stdout,
            read_buffer: Vec::new(),
            next_id: 1,
            snapshot_line_buffer: Vec::new(),
        })
    }

    fn initialize(&mut self) -> Result<Value, String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "agentmux-relay",
                    "title": "Agentmux Relay",
                },
            }),
            None,
            None,
        )
        .map_err(|error| match error {
            AcpRequestError::Failed(reason) => reason,
            AcpRequestError::Timeout(timeout) => {
                format!("ACP initialize timed out after {}ms", timeout.as_millis())
            }
        })
    }

    fn new_session(&mut self, working_directory: &Path) -> Result<String, String> {
        let result = self
            .request(
                "session/new",
                json!({
                    "cwd": working_directory.display().to_string(),
                }),
                None,
                None,
            )
            .map_err(|error| match error {
                AcpRequestError::Failed(reason) => reason,
                AcpRequestError::Timeout(timeout) => {
                    format!("ACP session/new timed out after {}ms", timeout.as_millis())
                }
            })?;
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| "ACP session/new response missing result.sessionId".to_string())
    }

    fn load_session(&mut self, session_id: &str, working_directory: &Path) -> Result<(), String> {
        let _ = self
            .request(
                "session/load",
                json!({
                    "sessionId": session_id,
                    "cwd": working_directory.display().to_string(),
                }),
                None,
                None,
            )
            .map_err(|error| match error {
                AcpRequestError::Failed(reason) => reason,
                AcpRequestError::Timeout(timeout) => {
                    format!("ACP session/load timed out after {}ms", timeout.as_millis())
                }
            })?;
        Ok(())
    }

    fn prompt(
        &mut self,
        session_id: &str,
        prompt: &str,
        timeout: Option<Duration>,
    ) -> Result<String, AcpRequestError> {
        let result = self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": prompt,
                    }
                ],
            }),
            timeout,
            Some(session_id),
        )?;
        result
            .get("stopReason")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                AcpRequestError::Failed(
                    "ACP session/prompt response missing result.stopReason".to_string(),
                )
            })
    }

    fn take_snapshot_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.snapshot_line_buffer)
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        prompt_session_id: Option<&str>,
    ) -> Result<Value, AcpRequestError> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let message = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }))
        .map_err(|source| {
            AcpRequestError::Failed(format!("serialize ACP request failed: {source}"))
        })?;
        self.stdin
            .write_all(message.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|source| {
                AcpRequestError::Failed(format!("write ACP request failed: {source}"))
            })?;

        loop {
            let line = self.read_response_line(timeout)?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let decoded = serde_json::from_str::<Value>(trimmed).map_err(|source| {
                AcpRequestError::Failed(format!("parse ACP response failed: {source}"))
            })?;
            if decoded.get("id") != Some(&json!(request_id)) {
                self.capture_update_snapshot_lines(&decoded, prompt_session_id);
                continue;
            }
            if let Some(error) = decoded.get("error") {
                return Err(AcpRequestError::Failed(error.to_string()));
            }
            return Ok(decoded.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn capture_update_snapshot_lines(&mut self, value: &Value, session_id: Option<&str>) {
        if value.get("method").and_then(Value::as_str) != Some("session/update") {
            return;
        }
        let params = value.get("params").unwrap_or(&Value::Null);
        if let Some(expected_session_id) = session_id
            && let Some(observed_session_id) = params.get("sessionId").and_then(Value::as_str)
            && observed_session_id != expected_session_id
        {
            return;
        }
        collect_text_lines_from_value(params, &mut self.snapshot_line_buffer);
    }

    fn read_response_line(&mut self, timeout: Option<Duration>) -> Result<String, AcpRequestError> {
        let deadline = timeout.map(|value| Instant::now() + value);
        let mut chunk = [0_u8; 4096];
        loop {
            if let Some(newline_index) = self.read_buffer.iter().position(|value| *value == b'\n') {
                let mut line = self.read_buffer.drain(..=newline_index).collect::<Vec<_>>();
                if matches!(line.last(), Some(b'\n')) {
                    line.pop();
                }
                if matches!(line.last(), Some(b'\r')) {
                    line.pop();
                }
                return String::from_utf8(line).map_err(|source| {
                    AcpRequestError::Failed(format!("decode ACP response failed: {source}"))
                });
            }

            match self.stdout.read(&mut chunk) {
                Ok(0) => {
                    let exit_code = self
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|status| status.code());
                    return Err(AcpRequestError::Failed(format!(
                        "ACP peer closed stdout (exit_code={exit_code:?})"
                    )));
                }
                Ok(count) => self.read_buffer.extend_from_slice(&chunk[..count]),
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(limit) = deadline
                        && Instant::now() >= limit
                    {
                        return Err(AcpRequestError::Timeout(
                            timeout.unwrap_or(Duration::from_millis(0)),
                        ));
                    }
                    if let Ok(Some(status)) = self.child.try_wait() {
                        return Err(AcpRequestError::Failed(format!(
                            "ACP peer exited before response (exit_code={:?})",
                            status.code()
                        )));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(AcpRequestError::Failed(format!(
                        "read ACP response failed: {source}"
                    )));
                }
            }
        }
    }
}

fn collect_text_lines_from_value(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_text_lines_from_value(value, output);
            }
        }
        Value::Object(values) => {
            if let Some(text) = values.get("text").and_then(Value::as_str) {
                append_text_lines(text, output);
            }
            for value in values.values() {
                collect_text_lines_from_value(value, output);
            }
        }
        _ => {}
    }
}

fn append_text_lines(text: &str, output: &mut Vec<String>) {
    for line in text.split('\n') {
        let normalized = line.trim_end_matches('\r');
        if !normalized.is_empty() {
            output.push(normalized.to_string());
        }
    }
}

fn set_nonblocking(file_descriptor: i32, enable: bool) -> Result<(), String> {
    // SAFETY: `fcntl` is called with a live file descriptor owned by this
    // process. The command and arguments follow libc contract.
    let flags = unsafe { libc::fcntl(file_descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let updated_flags = if enable {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    // SAFETY: `fcntl` receives the same valid descriptor and bitflag payload.
    let result = unsafe { libc::fcntl(file_descriptor, libc::F_SETFL, updated_flags) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

pub(super) fn aggregate_chat_status(results: &[ChatResult]) -> ChatStatus {
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

pub(super) fn prompt_batch_settings() -> PromptBatchSettings {
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

fn async_delivery_registry() -> &'static AsyncDeliveryRegistry {
    ASYNC_DELIVERY_REGISTRY.get_or_init(AsyncDeliveryRegistry::default)
}

fn async_worker_count() -> usize {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| workers.len())
        .unwrap_or(0)
}

fn spawn_async_delivery_worker(key: AsyncWorkerKey, receiver: mpsc::Receiver<AsyncDeliveryTask>) {
    thread::spawn(move || {
        loop {
            if shutdown_requested() {
                drop_pending_async_tasks_on_shutdown(&receiver);
                break;
            }
            let received =
                receiver.recv_timeout(Duration::from_millis(ASYNC_WORKER_POLL_INTERVAL_MS));
            let task = match received {
                Ok(task) => task,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if shutdown_requested() {
                emit_async_shutdown_drop(&task);
                drop_pending_async_tasks_on_shutdown(&receiver);
                break;
            }

            let outcome = deliver_one_target(&task);
            match outcome {
                Ok(result) => emit_inscription(
                    "relay.chat.async.completed",
                    &json!({
                        "bundle_name": task.bundle.bundle_name,
                        "sender_session": task.sender.id,
                        "target_session": result.target_session,
                        "message_id": result.message_id,
                        "outcome": result.outcome,
                        "reason_code": result.reason_code,
                        "reason": result.reason,
                        "details": result.details,
                    }),
                ),
                Err(error) => emit_inscription(
                    "relay.chat.async.completed",
                    &json!({
                        "bundle_name": task.bundle.bundle_name,
                        "sender_session": task.sender.id,
                        "target_session": task.target_session,
                        "message_id": task.message_id,
                        "outcome": ChatOutcome::Failed,
                        "reason": error.message,
                        "error_code": error.code,
                    }),
                ),
            }
        }
        if let Ok(mut workers) = async_delivery_registry().workers.lock() {
            workers.remove(&key);
        }
    });
}

fn drop_pending_async_tasks_on_shutdown(receiver: &mpsc::Receiver<AsyncDeliveryTask>) {
    while let Ok(task) = receiver.try_recv() {
        emit_async_shutdown_drop(&task);
    }
}

fn emit_async_shutdown_drop(task: &AsyncDeliveryTask) {
    emit_inscription(
        "relay.chat.async.completed",
        &json!({
            "bundle_name": task.bundle.bundle_name,
            "sender_session": task.sender.id,
            "target_session": task.target_session,
            "message_id": task.message_id,
            "outcome": ChatOutcome::DroppedOnShutdown,
            "reason_code": DROPPED_ON_SHUTDOWN_REASON_CODE,
            "reason": DROPPED_ON_SHUTDOWN_REASON,
        }),
    );
}

fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    options: QuiescenceOptions,
    prompt_readiness: Option<&PromptReadinessTemplate>,
) -> Result<String, DeliveryWaitError> {
    let readiness = build_prompt_readiness_matcher(prompt_readiness)
        .map_err(|reason| DeliveryWaitError::Failed { reason })?;
    let deadline = options
        .quiescence_timeout
        .map(|timeout| Instant::now() + timeout);
    let mut readiness_mismatch = false;
    let mut mismatch_reason = None::<String>;
    loop {
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }
        let pane_before = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_before = capture_pane_snapshot(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_before = resolve_window_activity_marker(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(options.quiet_window);
        if shutdown_requested() {
            return Err(DeliveryWaitError::Shutdown);
        }

        let pane_after = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_after = capture_pane_snapshot(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let activity_after = resolve_window_activity_marker(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let pane_is_quiescent = pane_before == pane_after
            && snapshot_before == snapshot_after
            && match (activity_before.as_ref(), activity_after.as_ref()) {
                (Some(before), Some(after)) => before == after,
                _ => true,
            };
        if pane_is_quiescent {
            if let Some(reason) =
                operator_interaction_active(tmux_socket, target_session, pane_after.as_str())
                    .map_err(|reason| DeliveryWaitError::Failed { reason })?
            {
                emit_delivery_diagnostic(
                    "delivery_operator_interaction",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                        "reason": reason,
                    }),
                );
                continue;
            }
            let evaluation = match prompt_readiness_matches(
                tmux_socket,
                pane_after.as_str(),
                snapshot_after.as_str(),
                readiness.as_ref(),
            ) {
                Ok(evaluation) => evaluation,
                Err(reason) => return Err(DeliveryWaitError::Failed { reason }),
            };
            if evaluation.ready {
                emit_delivery_diagnostic(
                    "delivery_ready",
                    &json!({
                        "target_session": target_session,
                        "pane_target": pane_after,
                    }),
                );
                return Ok(pane_after);
            }
            readiness_mismatch = true;
            mismatch_reason = evaluation.mismatch_reason.clone();
            emit_delivery_diagnostic(
                "delivery_prompt_mismatch",
                &json!({
                    "target_session": target_session,
                    "pane_target": pane_after,
                    "mismatch_reason": evaluation.mismatch_reason,
                    "regex_matched": evaluation.regex_matched,
                    "inspected_block": evaluation.inspected_block,
                    "expected_cursor_column": evaluation.expected_cursor_column,
                    "observed_cursor_column": evaluation.observed_cursor_column,
                }),
            );
        }

        if deadline.is_some_and(|value| Instant::now() >= value) {
            let timeout = options.quiescence_timeout.unwrap_or_default();
            emit_delivery_diagnostic(
                "quiescence_timeout",
                &json!({
                    "target_session": target_session,
                    "quiescence_timeout_ms": timeout.as_millis(),
                    "readiness_mismatch": readiness_mismatch,
                    "mismatch_reason": mismatch_reason,
                }),
            );
            return Err(DeliveryWaitError::Timeout {
                timeout,
                readiness_mismatch,
                mismatch_reason,
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
) -> Result<PromptReadinessEvaluation, String> {
    let Some(matcher) = matcher else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            ..PromptReadinessEvaluation::default()
        });
    };

    let inspected = snapshot
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(matcher.inspect_lines)
        .collect::<Vec<_>>();
    if inspected.is_empty() {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(
                "inspected pane tail was empty after trimming trailing blank lines".to_string(),
            ),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }
    let mut ordered = inspected;
    ordered.reverse();
    let block = ordered.join("\n");
    if !matcher.prompt_regex.is_match(block.as_str()) {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some("prompt regex did not match inspected pane tail".to_string()),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(false),
            expected_cursor_column: matcher.input_idle_cursor_column,
            ..PromptReadinessEvaluation::default()
        });
    }

    let Some(expected_cursor_column) = matcher.input_idle_cursor_column else {
        return Ok(PromptReadinessEvaluation {
            ready: true,
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            ..PromptReadinessEvaluation::default()
        });
    };
    let cursor_column = resolve_cursor_column(tmux_socket, pane_target)?;
    if cursor_column != expected_cursor_column {
        return Ok(PromptReadinessEvaluation {
            mismatch_reason: Some(format!(
                "cursor column {} did not match required {}",
                cursor_column, expected_cursor_column
            )),
            inspected_block: Some(sanitize_diagnostic_text(&block)),
            regex_matched: Some(true),
            expected_cursor_column: Some(expected_cursor_column),
            observed_cursor_column: Some(cursor_column),
            ..PromptReadinessEvaluation::default()
        });
    }

    Ok(PromptReadinessEvaluation {
        ready: true,
        inspected_block: Some(sanitize_diagnostic_text(&block)),
        regex_matched: Some(true),
        expected_cursor_column: Some(expected_cursor_column),
        observed_cursor_column: Some(cursor_column),
        ..PromptReadinessEvaluation::default()
    })
}
