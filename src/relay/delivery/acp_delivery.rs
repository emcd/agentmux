use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};

use crate::configuration::{AcpTargetConfiguration, BundleMember, TargetConfiguration};

use super::acp_client::{AcpStdioClient, PromptCompletion, PromptDispatchOutcome};
use super::acp_state::{load_persisted_acp_session_id, persist_acp_session_id};
use super::async_worker::{AcpWorkerReadinessState, get_acp_worker_state, set_acp_worker_state};
use super::results::{
    delivered_in_progress_result, delivered_result, failed_result, failed_result_with_code,
};
use super::{
    PermissionEventContext, PermissionResolutionOutcome, enqueue_permission_request,
    wait_for_permission_resolution,
};

use super::super::{AsyncDeliveryTask, SendResult};

// ACP delivery failure taxonomy:
// - `runtime_acp_initialize_failed` / `_session_new_failed` / `_session_load_failed`:
//   a logical error returned by the ACP agent during bootstrap (well-formed JSON-RPC
//   error response or unexpected response shape).
// - `runtime_acp_prompt_failed`: a logical error returned by the ACP agent during
//   `session/prompt` (JSON-RPC error response or unsupported `stopReason`).
// - `runtime_acp_connection_closed`: ACP stdout reached EOF before any first
//   activity for the active prompt; child likely exited cleanly.
// - `acp_child_unavailable`: ACP child stdin write failed (broken pipe, child
//   crashed, transport-level fault). Worker is transitioned to `Unavailable`.
// - `acp_turn_timeout`: relay-imposed pre-first-activity timeout elapsed.
// - `acp_stop_cancelled`: prompt completed with `stopReason=cancelled`.
// - `validation_missing_acp_capability`: agent did not advertise required
//   capability (`promptSession`, `loadSession`).
// TODO(refactor-acp-background-reader follow-up): `acp_turn_timeout` is no
// longer emitted; relay-side turn timeout is removed (D4). Kept declared
// only so external consumers reading reason codes still have a reference.
#[allow(dead_code)]
pub(super) const ACP_REASON_CODE_TURN_TIMEOUT: &str = "acp_turn_timeout";
pub(super) const ACP_REASON_CODE_STOP_CANCELLED: &str = "acp_stop_cancelled";
pub(super) const ACP_ERROR_CODE_INITIALIZE_FAILED: &str = "runtime_acp_initialize_failed";
pub(super) const ACP_ERROR_CODE_SESSION_LOAD_FAILED: &str = "runtime_acp_session_load_failed";
pub(super) const ACP_ERROR_CODE_SESSION_NEW_FAILED: &str = "runtime_acp_session_new_failed";
pub(super) const ACP_ERROR_CODE_PROMPT_FAILED: &str = "runtime_acp_prompt_failed";
pub(super) const ACP_ERROR_CODE_CONNECTION_CLOSED: &str = "runtime_acp_connection_closed";
pub(super) const ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE: &str = "acp_child_unavailable";
pub(super) const ACP_ERROR_CODE_MISSING_CAPABILITY: &str = "validation_missing_acp_capability";

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

pub(super) struct PersistentAcpWorkerRuntime {
    pub client: AcpStdioClient,
    pub session_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct AcpBootstrapError {
    pub code: String,
    pub reason: String,
}

impl AcpBootstrapError {
    // Permanent bootstrap failures are conditions that respawn cannot resolve.
    // Capability gaps mean the agent fundamentally cannot host the session,
    // so retrying with the same binary would just reproduce the failure.
    pub fn is_permanent(&self) -> bool {
        self.code == ACP_ERROR_CODE_MISSING_CAPABILITY
    }
}

pub(super) fn bootstrap_acp_worker_runtime(
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let TargetConfiguration::Acp(acp_target) = &target_member.target else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires ACP target".to_string(),
        });
    };
    let Some(working_directory) = target_member.working_directory.as_ref() else {
        return Err(AcpBootstrapError {
            code: "runtime_startup_failed".to_string(),
            reason: "ACP worker bootstrap requires target working directory".to_string(),
        });
    };
    let target_session = target_member.id.as_str();
    let message_id = "acp-worker-bootstrap";
    initialize_persistent_acp_worker_runtime(
        target_member,
        acp_target,
        working_directory,
        runtime_directory,
        target_session,
        message_id,
    )
    .map_err(|result| AcpBootstrapError {
        code: result
            .reason_code
            .clone()
            .unwrap_or_else(|| "runtime_startup_failed".to_string()),
        reason: result
            .reason
            .clone()
            .unwrap_or_else(|| "ACP worker bootstrap failed".to_string()),
    })
}

// Rebuilds the per-target ACP runtime after a transport failure and refreshes
// the worker registry's replay buffer handle so future Look queries observe
// the new child's stream rather than the dead one. Surfacing the structured
// `AcpBootstrapError` lets the worker loop decide whether to keep retrying or
// give up permanently.
pub(super) fn respawn_acp_worker_runtime(
    bundle_name: &str,
    runtime_directory: &Path,
    target_member: &BundleMember,
) -> Result<PersistentAcpWorkerRuntime, AcpBootstrapError> {
    let runtime = bootstrap_acp_worker_runtime(runtime_directory, target_member)?;
    super::async_worker::install_acp_worker_replay_buffer(
        bundle_name,
        runtime_directory,
        target_member.id.as_str(),
        runtime.client.replay_buffer_handle(),
    );
    Ok(runtime)
}

/// Delivers a coalesced envelope batch over one ACP prompt.
///
/// Mirrors `deliver_one_target_acp` but folds N tasks into the prompt's
/// completion path: the on_completion callback fans the single transport
/// outcome out to every task's `completion_sender` using each task's own
/// `target_session` and `message_id`. The shared permission decision window
/// (one tool-call surface per coalesced prompt) is correlated to the head
/// task's `message_id`; that is the operator-facing message under which the
/// decision is recorded.
pub(super) fn deliver_batch_target_acp(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    _acp: &AcpTargetConfiguration,
    prompt_batches: Vec<String>,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> Vec<SendResult> {
    debug_assert!(!batch.is_empty());
    let head = &batch[0];
    let head_target_session = head.target_session.clone();
    let head_message_id = head.message_id.clone();
    if target_member.working_directory.is_none() {
        return replicate_failed_result(
            batch,
            "ACP target is missing working directory".to_string(),
        );
    }
    let runtime_directory = head.runtime_directory.as_path();

    if matches!(
        get_acp_worker_state(
            head.bundle.bundle_name.as_str(),
            runtime_directory,
            target_member.id.as_str(),
        ),
        Some(AcpWorkerReadinessState::Unavailable)
    ) {
        return replicate_failed_result_with_code(
            batch,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
        );
    }

    let Some(runtime) = acp_runtime.as_mut() else {
        return replicate_failed_result_with_code(
            batch,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
        );
    };

    debug_assert!(
        prompt_batches.len() == 1,
        "ACP batch delivery expects exactly one prompt batch; payload preparation peels the tail when more are produced",
    );
    let Some(prompt) = prompt_batches.into_iter().next() else {
        return replicate_failed_result(batch, "ACP delivery received no prompt batch".to_string());
    };

    let permission_context = PermissionEventContext {
        runtime_directory: head.runtime_directory.clone(),
        bundle_name: head.bundle.bundle_name.clone(),
        // The head's decider list governs the permission decision for the
        // coalesced prompt. All tasks share the same target session, so the
        // permission_decider_sessions field will typically match across tasks;
        // diverging values would indicate a configuration race we do not
        // attempt to reconcile here.
        authorized_ui_sessions: head.permission_decider_sessions.clone(),
    };
    let pending_permission_outcome_shared: Arc<Mutex<Option<PermissionResolutionOutcome>>> =
        Arc::new(Mutex::new(None));

    let bundle_name = head.bundle.bundle_name.clone();
    let runtime_directory_owned = head.runtime_directory.clone();
    let target_member_id = target_member.id.clone();
    let session_id = runtime.session_id.clone();

    let dispatch_bundle_name = bundle_name.clone();
    let dispatch_runtime_directory = runtime_directory_owned.clone();
    let dispatch_target_member_id = target_member_id.clone();
    let on_dispatched: crate::acp::DispatchHandler = Box::new(move || {
        set_acp_worker_state(
            dispatch_bundle_name.as_str(),
            dispatch_runtime_directory.as_path(),
            dispatch_target_member_id.as_str(),
            AcpWorkerReadinessState::Busy,
        );
    });

    let permission_context_for_handler = permission_context.clone();
    // Permission requests inside one ACP prompt correlate to the head
    // message_id; downstream UIs can resolve the batch via inscriptions if
    // they need to enumerate the coalesced message ids.
    let permission_message_id_for_handler = head_message_id.clone();
    let permission_target_member_id = target_member_id.clone();
    let permission_max_pending = head.permission_max_pending;
    let pending_permission_outcome_writer = Arc::clone(&pending_permission_outcome_shared);
    let on_permission_request: crate::acp::PermissionHandler = Box::new(
        move |permission_request: crate::acp::PermissionRequest,
              mut responder: crate::acp::PermissionResponder| {
            // Resolver runs on a dedicated short-lived thread so the ACP
            // reader thread can return to its `read_line` loop immediately
            // (see todos/acp/16). The captured context/writer/responder
            // outlive the reader's dispatch frame; the thread name aids
            // debugging when many resolvers fire under federation load.
            let context = permission_context_for_handler.clone();
            let message_id = permission_message_id_for_handler.clone();
            let target = permission_target_member_id.clone();
            let writer = Arc::clone(&pending_permission_outcome_writer);
            std::thread::Builder::new()
                .name("acp-permission-resolver".to_string())
                .spawn(move || {
                    let (response_option_id, outcome) = resolve_acp_permission_request(
                        &context,
                        message_id.as_str(),
                        target.as_str(),
                        &permission_request,
                        permission_max_pending,
                    );
                    // Ordering invariant: populate
                    // pending_permission_outcome BEFORE writing the
                    // JSON-RPC response. on_completion reads the shared
                    // slot when building the final SendResult; the agent
                    // cannot send its prompt-response until it sees the
                    // permission response, so once the responder writes,
                    // a fast agent reply could race on_completion ahead
                    // of the outcome record otherwise.
                    *writer.lock().expect("pending_permission_outcome mutex") = Some(outcome);
                    responder.respond(response_option_id);
                })
                .expect("spawn ACP permission resolver thread");
        },
    );

    // Per-task correlation captured into on_completion. One ACP completion
    // outcome maps to N synthesised per-task SendResults so each original
    // send call's completion_sender receives a result tied to its own
    // message_id and target_session.
    let completion_correlations: Vec<TaskCorrelation> = batch
        .iter()
        .map(|task| TaskCorrelation {
            completion_sender: task.completion_sender.clone(),
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
        })
        .collect();
    let completion_bundle_name = bundle_name.clone();
    let completion_runtime_directory = runtime_directory_owned.clone();
    let completion_target_member_id = target_member_id.clone();
    let pending_permission_outcome_reader = Arc::clone(&pending_permission_outcome_shared);
    let on_completion: crate::acp::PromptCompletionHandler = Box::new(move |completion| {
        let pending_permission_outcome = pending_permission_outcome_reader
            .lock()
            .expect("pending_permission_outcome mutex")
            .clone();
        // build_acp_completion_result is deterministic over (completion,
        // pending_permission_outcome); the final state and the non-correlation
        // fields of the result are identical across tasks. Compute the
        // template once (consuming `completion`, which is not Clone), then
        // replicate per task by overwriting target_session and message_id.
        let template_correlation = completion_correlations
            .first()
            .expect("batch ACP completion requires at least one correlation");
        let (final_state, template_result) = build_acp_completion_result(
            completion,
            pending_permission_outcome,
            template_correlation.target_session.clone(),
            template_correlation.message_id.clone(),
            completion_target_member_id.as_str(),
        );
        for correlation in &completion_correlations {
            let mut per_task = template_result.clone();
            per_task.target_session = correlation.target_session.clone();
            per_task.message_id = correlation.message_id.clone();
            if let Some(sender) = correlation.completion_sender.as_ref() {
                let _ = sender.send(Ok(per_task));
            }
        }
        set_acp_worker_state(
            completion_bundle_name.as_str(),
            completion_runtime_directory.as_path(),
            completion_target_member_id.as_str(),
            final_state,
        );
    });

    let outcome = runtime.client.prompt(
        session_id.as_str(),
        prompt.as_str(),
        Some(on_dispatched),
        Some(on_permission_request),
        on_completion,
    );

    match outcome {
        PromptDispatchOutcome::Submitted => batch
            .iter()
            .map(|task| {
                delivered_in_progress_result(task.target_session.clone(), task.message_id.clone())
            })
            .collect(),
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            // Touch the head's correlation values so the head-derived error
            // matches the single-task path's failed_result_with_code shape
            // verbatim; the per-task results are replicated below.
            let _ = (head_target_session, head_message_id);
            replicate_failed_result_with_code(
                batch,
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            replicate_failed_result_with_code(
                batch,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt dispatch failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
    }
}

#[derive(Clone)]
struct TaskCorrelation {
    completion_sender:
        Option<std::sync::mpsc::Sender<Result<SendResult, crate::relay::RelayError>>>,
    target_session: String,
    message_id: String,
}

fn replicate_failed_result(batch: &[AsyncDeliveryTask], reason: String) -> Vec<SendResult> {
    batch
        .iter()
        .map(|task| {
            failed_result(
                task.target_session.clone(),
                task.message_id.clone(),
                reason.clone(),
            )
        })
        .collect()
}

fn replicate_failed_result_with_code(
    batch: &[AsyncDeliveryTask],
    code: &'static str,
    reason: &'static str,
    details: Option<Value>,
) -> Vec<SendResult> {
    batch
        .iter()
        .map(|task| {
            failed_result_with_code(
                task.target_session.clone(),
                task.message_id.clone(),
                code,
                reason,
                details.clone(),
            )
        })
        .collect()
}

pub(super) fn deliver_one_target_acp(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    _acp: &AcpTargetConfiguration,
    prompt_batches: Vec<String>,
    target_session: String,
    message_id: String,
    acp_runtime: &mut Option<PersistentAcpWorkerRuntime>,
) -> SendResult {
    if target_member.working_directory.is_none() {
        return failed_result(
            target_session,
            message_id,
            "ACP target is missing working directory",
        );
    }
    let runtime_directory = task.runtime_directory.as_path();

    // Fail fast if the background reader already observed a transport
    // failure (Unavailable state). Without this, a dispatch would proceed
    // to write to a dead pipe and bubble up as `acp_child_unavailable`,
    // muddling the "worker is broken" signal. TODO(acp): consider
    // graceful auto-respawn here; see todos/acp/auto_respawn_after_transport_failure.
    if matches!(
        get_acp_worker_state(
            task.bundle.bundle_name.as_str(),
            runtime_directory,
            target_member.id.as_str(),
        ),
        Some(AcpWorkerReadinessState::Unavailable)
    ) {
        return failed_result_with_code(
            target_session,
            message_id,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
        );
    }

    let Some(runtime) = acp_runtime.as_mut() else {
        return failed_result_with_code(
            target_session,
            message_id,
            "runtime_acp_worker_unavailable",
            "ACP worker is unavailable for target session",
            Some(json!({
                "target_session": target_member.id,
            })),
        );
    };

    debug_assert!(
        prompt_batches.len() == 1,
        "ACP delivery expects exactly one prompt batch; multi-batch requires chaining via on_completion (not implemented)"
    );
    let Some(prompt) = prompt_batches.into_iter().next() else {
        return failed_result(
            target_session,
            message_id,
            "ACP delivery received no prompt batch",
        );
    };

    let permission_context = PermissionEventContext {
        runtime_directory: task.runtime_directory.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        authorized_ui_sessions: task.permission_decider_sessions.clone(),
    };
    let pending_permission_outcome_shared: Arc<Mutex<Option<PermissionResolutionOutcome>>> =
        Arc::new(Mutex::new(None));

    let bundle_name = task.bundle.bundle_name.clone();
    let runtime_directory_owned = task.runtime_directory.clone();
    let target_member_id = target_member.id.clone();
    let session_id = runtime.session_id.clone();

    let dispatch_bundle_name = bundle_name.clone();
    let dispatch_runtime_directory = runtime_directory_owned.clone();
    let dispatch_target_member_id = target_member_id.clone();
    let on_dispatched: crate::acp::DispatchHandler = Box::new(move || {
        set_acp_worker_state(
            dispatch_bundle_name.as_str(),
            dispatch_runtime_directory.as_path(),
            dispatch_target_member_id.as_str(),
            AcpWorkerReadinessState::Busy,
        );
    });

    let permission_context_for_handler = permission_context.clone();
    let message_id_for_handler = message_id.clone();
    let permission_target_member_id = target_member_id.clone();
    let permission_max_pending = task.permission_max_pending;
    let pending_permission_outcome_writer = Arc::clone(&pending_permission_outcome_shared);
    let on_permission_request: crate::acp::PermissionHandler = Box::new(
        move |permission_request: crate::acp::PermissionRequest,
              mut responder: crate::acp::PermissionResponder| {
            // See the matching comment in deliver_batch_target_acp: the
            // resolver runs on a short-lived thread so the ACP reader is
            // never parked, and the ordering invariant (outcome before
            // response) is the same.
            let context = permission_context_for_handler.clone();
            let message_id = message_id_for_handler.clone();
            let target = permission_target_member_id.clone();
            let writer = Arc::clone(&pending_permission_outcome_writer);
            std::thread::Builder::new()
                .name("acp-permission-resolver".to_string())
                .spawn(move || {
                    let (response_option_id, outcome) = resolve_acp_permission_request(
                        &context,
                        message_id.as_str(),
                        target.as_str(),
                        &permission_request,
                        permission_max_pending,
                    );
                    *writer.lock().expect("pending_permission_outcome mutex") = Some(outcome);
                    responder.respond(response_option_id);
                })
                .expect("spawn ACP permission resolver thread");
        },
    );

    let completion_bundle_name = bundle_name.clone();
    let completion_runtime_directory = runtime_directory_owned.clone();
    let completion_target_member_id = target_member_id.clone();
    let completion_target_session = target_session.clone();
    let completion_message_id = message_id.clone();
    let completion_sender = task.completion_sender.clone();
    let pending_permission_outcome_reader = Arc::clone(&pending_permission_outcome_shared);
    let on_completion: crate::acp::PromptCompletionHandler = Box::new(move |completion| {
        let pending_permission_outcome = pending_permission_outcome_reader
            .lock()
            .expect("pending_permission_outcome mutex")
            .clone();
        let (final_state, final_result) = build_acp_completion_result(
            completion,
            pending_permission_outcome,
            completion_target_session.clone(),
            completion_message_id.clone(),
            completion_target_member_id.as_str(),
        );
        set_acp_worker_state(
            completion_bundle_name.as_str(),
            completion_runtime_directory.as_path(),
            completion_target_member_id.as_str(),
            final_state,
        );
        if let Some(sender) = completion_sender.as_ref() {
            let _ = sender.send(Ok(final_result));
        }
    });

    let outcome = runtime.client.prompt(
        session_id.as_str(),
        prompt.as_str(),
        Some(on_dispatched),
        Some(on_permission_request),
        on_completion,
    );

    match outcome {
        PromptDispatchOutcome::Submitted => {
            delivered_in_progress_result(target_session, message_id)
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE,
                "ACP child stdin write failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            set_acp_worker_state(
                bundle_name.as_str(),
                runtime_directory,
                target_member.id.as_str(),
                AcpWorkerReadinessState::Unavailable,
            );
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt dispatch failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )
        }
    }
}

fn build_acp_completion_result(
    completion: PromptCompletion,
    pending_permission_outcome: Option<PermissionResolutionOutcome>,
    target_session: String,
    message_id: String,
    target_member_id: &str,
) -> (AcpWorkerReadinessState, SendResult) {
    if let Some(PermissionResolutionOutcome::Cancelled {
        reason_code,
        reason,
        ..
    }) = pending_permission_outcome
    {
        return (
            AcpWorkerReadinessState::Available,
            failed_result_with_code(
                target_session,
                message_id,
                reason_code.as_str(),
                reason.unwrap_or_else(|| "ACP permission request was cancelled".to_string()),
                Some(json!({
                    "target_session": target_member_id,
                })),
            ),
        );
    }

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => (
                AcpWorkerReadinessState::Available,
                delivered_result(target_session, message_id),
            ),
            "cancelled" => (
                AcpWorkerReadinessState::Available,
                failed_result_with_code(
                    target_session,
                    message_id,
                    ACP_REASON_CODE_STOP_CANCELLED,
                    "ACP turn completed with stopReason=cancelled",
                    None,
                ),
            ),
            other => (
                AcpWorkerReadinessState::Available,
                failed_result(
                    target_session,
                    message_id,
                    format!("ACP returned unsupported stopReason '{other}'"),
                ),
            ),
        },
        PromptCompletion::ProtocolError(reason) => (
            AcpWorkerReadinessState::Available,
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_PROMPT_FAILED,
                "ACP session/prompt failed",
                Some(json!({
                    "target_session": target_member_id,
                    "reason": reason,
                })),
            ),
        ),
        PromptCompletion::ConnectionClosed { reason } => (
            AcpWorkerReadinessState::Unavailable,
            failed_result_with_code(
                target_session,
                message_id,
                ACP_ERROR_CODE_CONNECTION_CLOSED,
                "ACP connection closed before prompt response",
                Some(json!({
                    "target_session": target_member_id,
                    "reason": reason,
                })),
            ),
        ),
    }
}

fn resolve_acp_permission_request(
    permission_context: &PermissionEventContext,
    message_id: &str,
    target_session: &str,
    permission_request: &crate::acp::PermissionRequest,
    permission_max_pending: usize,
) -> (Option<String>, PermissionResolutionOutcome) {
    let requested_details = json!({
        "tool_call_title": permission_request.tool_call_title.clone(),
        "options": permission_request.options.clone(),
        "acp_request_id": permission_request.request_id,
        "raw": permission_request.requested_details.clone(),
    });
    let enqueue = enqueue_permission_request(
        permission_context,
        message_id,
        target_session,
        permission_request.requested_kind.as_str(),
        requested_details,
        permission_max_pending,
    );
    let enqueued = match enqueue {
        Ok(value) => value,
        Err(code) if code == "runtime_permission_queue_full" => {
            return (
                None,
                PermissionResolutionOutcome::Cancelled {
                    decided_by: "relay".to_string(),
                    reason_code: "runtime_permission_queue_full".to_string(),
                    reason: Some("permission queue is full".to_string()),
                },
            );
        }
        Err(_) => {
            return (
                None,
                PermissionResolutionOutcome::Cancelled {
                    decided_by: "relay".to_string(),
                    reason_code: "runtime_permission_queue_unavailable".to_string(),
                    reason: Some("failed to enqueue permission request".to_string()),
                },
            );
        }
    };

    let outcome =
        wait_for_permission_resolution(permission_context, enqueued.permission_request_id.as_str());
    let Ok(outcome) = outcome else {
        return (
            None,
            PermissionResolutionOutcome::Cancelled {
                decided_by: "relay".to_string(),
                reason_code: "runtime_permission_request_cancelled".to_string(),
                reason: Some("failed while waiting for permission decision".to_string()),
            },
        );
    };

    let response_option_id = match &outcome {
        PermissionResolutionOutcome::Selected { option_id, .. } => Some(option_id.clone()),
        PermissionResolutionOutcome::Cancelled { .. } => None,
    };
    (response_option_id, outcome)
}

fn initialize_persistent_acp_worker_runtime(
    target_member: &BundleMember,
    acp: &AcpTargetConfiguration,
    working_directory: &Path,
    runtime_directory: &Path,
    target_session: &str,
    message_id: &str,
) -> Result<PersistentAcpWorkerRuntime, Box<SendResult>> {
    let mut client = match acp.channel {
        crate::configuration::AcpChannel::Stdio => {
            let Some(command) = acp.command.as_deref() else {
                return Err(Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    "ACP stdio target requires command",
                )));
            };
            AcpStdioClient::spawn(
                command,
                working_directory,
                &acp.environment
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect::<Vec<_>>(),
            )
            .map_err(|reason| {
                Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    reason,
                ))
            })?
        }
        crate::configuration::AcpChannel::Http => {
            return Err(Box::new(failed_result(
                target_session.to_string(),
                message_id.to_string(),
                "ACP http transport is not implemented",
            )));
        }
    };

    let initialize_result = match client.initialize() {
        Ok(value) => value,
        Err(reason) => {
            return Err(Box::new(failed_result_with_code(
                target_session.to_string(),
                message_id.to_string(),
                ACP_ERROR_CODE_INITIALIZE_FAILED,
                "ACP initialize failed",
                Some(json!({
                    "target_session": target_member.id,
                    "reason": reason,
                })),
            )));
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
            .map(|value| {
                value
                    .get("promptSession")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        value
                            .get("promptCapabilities")
                            .is_some_and(serde_json::Value::is_object)
                    })
            })
            .unwrap_or(false),
    };

    let persisted_session_id = if target_member.coder_session_id.is_some() {
        None
    } else {
        load_persisted_acp_session_id(runtime_directory, target_member.id.as_str()).map_err(
            |reason| {
                Box::new(failed_result(
                    target_session.to_string(),
                    message_id.to_string(),
                    format!("failed to load persisted ACP session id: {reason}"),
                ))
            },
        )?
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
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_MISSING_CAPABILITY,
                    "ACP agent does not advertise required load capability",
                    Some(json!({
                        "target_session": target_member.id,
                        "required_capability": "session/load",
                        "reason": "agentCapabilities.loadSession is false or missing",
                    })),
                )));
            }
            if let Err(reason) =
                client.load_session(lifecycle_session_id.as_str(), working_directory)
            {
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_SESSION_LOAD_FAILED,
                    "ACP session/load failed",
                    Some(json!({
                        "target_session": target_member.id,
                        "session_id": lifecycle_session_id,
                        "reason": reason,
                    })),
                )));
            }
            lifecycle_session_id
        }
        AcpLifecycleSelection::NewSession => match client.new_session(working_directory) {
            Ok(value) => value,
            Err(reason) => {
                return Err(Box::new(failed_result_with_code(
                    target_session.to_string(),
                    message_id.to_string(),
                    ACP_ERROR_CODE_SESSION_NEW_FAILED,
                    "ACP session/new failed",
                    Some(json!({
                        "target_session": target_member.id,
                        "reason": reason,
                    })),
                )));
            }
        },
    };

    if let Err(reason) = persist_acp_session_id(
        runtime_directory,
        target_member.id.as_str(),
        session_id.as_str(),
    ) {
        return Err(Box::new(failed_result(
            target_session.to_string(),
            message_id.to_string(),
            format!("failed to persist ACP session id: {reason}"),
        )));
    }

    if !capabilities.prompt_session {
        return Err(Box::new(failed_result_with_code(
            target_session.to_string(),
            message_id.to_string(),
            ACP_ERROR_CODE_MISSING_CAPABILITY,
            "ACP agent does not advertise required prompt capability",
            Some(json!({
                "target_session": target_member.id,
                "required_capability": "session/prompt",
                "reason": "agentCapabilities.promptSession is false or missing",
            })),
        )));
    }

    Ok(PersistentAcpWorkerRuntime { client, session_id })
}
