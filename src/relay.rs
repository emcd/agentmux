//! Relay IPC contract and message-routing implementation.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock, mpsc},
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
    runtime::{
        inscriptions::emit_inscription, paths::BundleRuntimePaths, signals::shutdown_requested,
    },
};

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const DEFAULT_QUIET_WINDOW_MS: u64 = 750;
const DEFAULT_QUIESCENCE_TIMEOUT_MS: u64 = 30_000;
const OWNERSHIP_OPTION_NAME: &str = "@agentmux_owned";
const OWNERSHIP_OPTION_VALUE: &str = "1";
const CREATE_MAX_ATTEMPTS: usize = 4;
const CREATE_RETRY_BASE_DELAY_MS: u64 = 35;
const CREATE_RETRY_JITTER_MS: u64 = 35;
const MAX_PROMPT_TOKENS_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";
const DEFAULT_PROMPT_INSPECT_LINES: usize = 3;
const MAX_PROMPT_INSPECT_LINES: usize = 40;
const DELIVERY_DIAGNOSTICS_ENVVAR: &str = "AGENTMUX_RELAY_DELIVERY_DIAGNOSTICS";
const ASYNC_WORKER_POLL_INTERVAL_MS: u64 = 100;
const ASYNC_SHUTDOWN_WAIT_POLL_MS: u64 = 25;
const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";

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
            delivery_mode,
            quiet_window_ms,
            quiescence_timeout_ms,
        } => handle_chat(
            &bundle,
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
                "sender_session": sender_session,
                "recipient_count": recipients.len(),
            }),
        );
    }
    response
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

fn deliver_one_target(task: &AsyncDeliveryTask) -> Result<ChatResult, RelayError> {
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
                body: message.to_string(),
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
                None => Ok(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Delivered,
                    reason: None,
                }),
                Some(reason) => Ok(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Failed,
                    reason: Some(reason),
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
                reason: Some(reason),
            })
        }
        Err(DeliveryWaitError::Failed { reason }) => Ok(ChatResult {
            target_session,
            message_id,
            outcome: ChatOutcome::Failed,
            reason: Some(reason),
        }),
        Err(DeliveryWaitError::Shutdown) => Ok(ChatResult {
            target_session,
            message_id,
            outcome: ChatOutcome::DroppedOnShutdown,
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
        }),
    }
}

fn async_delivery_registry() -> &'static AsyncDeliveryRegistry {
    ASYNC_DELIVERY_REGISTRY.get_or_init(AsyncDeliveryRegistry::default)
}

/// Waits for async delivery workers to stop after shutdown is requested.
///
/// Returns the number of workers still running when timeout is reached.
#[must_use]
pub fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
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

fn async_worker_count() -> usize {
    async_delivery_registry()
        .workers
        .lock()
        .map(|workers| workers.len())
        .unwrap_or(0)
}

fn enqueue_async_delivery(task: AsyncDeliveryTask) -> Result<(), RelayError> {
    let key = AsyncWorkerKey {
        tmux_socket: task.tmux_socket.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        target_session: task.target_session.clone(),
    };
    let registry = async_delivery_registry();
    let mut workers = registry.workers.lock().map_err(|_| {
        relay_error(
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
        relay_error(
            "internal_unexpected_failure",
            "failed to enqueue async delivery task",
            Some(json!({"cause": source.to_string()})),
        )
    })?;
    spawn_async_delivery_worker(key.clone(), receiver);
    workers.insert(key, sender);
    Ok(())
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
                        "reason": result.reason,
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
            "reason": DROPPED_ON_SHUTDOWN_REASON,
        }),
    );
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
    let output = run_tmux_command_capture(tmux_socket, &["kill-server"]).map_err(|reason| {
        relay_error(
            "internal_unexpected_failure",
            "failed to clean up tmux socket",
            Some(json!({"cause": reason})),
        )
    })?;
    if output.status.success() {
        return Ok(true);
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if reason.to_ascii_lowercase().contains("no server running") {
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
    fn for_sync(quiet_window_ms: Option<u64>, quiescence_timeout_ms: Option<u64>) -> Self {
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

    fn for_async(quiet_window_ms: Option<u64>, quiescence_timeout_ms: Option<u64>) -> Self {
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

fn resolve_window_activity_marker(
    tmux_socket: &Path,
    pane_target: &str,
) -> Result<Option<String>, String> {
    let output = run_tmux_command_capture(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            pane_target,
            "#{window_activity}",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("unknown format")
            || lower.contains("invalid format")
            || lower.contains("bad format")
        {
            return Ok(None);
        }
        if stderr.is_empty() {
            return Err("tmux display-message for window_activity failed".to_string());
        }
        return Err(stderr);
    }
    let marker = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if marker.is_empty() {
        return Ok(None);
    }
    Ok(Some(marker))
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
    run_tmux_command(tmux_socket, &["send-keys", "-t", pane_target, "--", prompt])?;
    run_tmux_command(tmux_socket, &["send-keys", "-t", pane_target, "Enter"])?;
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
    std::env::var("AGENTMUX_TMUX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "tmux".to_string())
}

fn sanitize_diagnostic_text(text: &str) -> String {
    const MAX_CHARS: usize = 512;
    let mut clipped = text.chars().take(MAX_CHARS).collect::<String>();
    if text.chars().count() > MAX_CHARS {
        clipped.push_str("...");
    }
    clipped
}

fn emit_delivery_diagnostic(event: &str, details: &Value) {
    if !delivery_diagnostics_enabled() {
        return;
    }
    emit_inscription(format!("relay.{event}").as_str(), details);
}

fn delivery_diagnostics_enabled() -> bool {
    std::env::var(DELIVERY_DIAGNOSTICS_ENVVAR)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
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
