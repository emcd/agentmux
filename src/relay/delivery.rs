use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::{
    configuration::PromptReadinessTemplate,
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
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

    match wait_for_quiescent_pane(
        tmux_socket,
        &target_session,
        quiescence,
        target_member.prompt_readiness.as_ref(),
    ) {
        Ok(pane_target) => {
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
