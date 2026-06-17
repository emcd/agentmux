use crate::acp::AcpTransport;
use crate::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
use crate::tmux::TmuxTransport;
use crate::tmux::transport::{DeliveryWaitError, wait_for_quiescent_pane};
use crate::transports::{DeliveryContext, DeliveryEnvelope, DeliveryResult, Transport};

use super::super::super::startup_state::note_session_served_successfully;
use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SendOutcome, SendResult,
};

const DROPPED_ON_SHUTDOWN_REASON: &str = "relay shutdown requested before delivery";
const DROPPED_ON_SHUTDOWN_REASON_CODE: &str = "dropped_on_shutdown";

pub(super) fn deliver_non_ui_target(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Result<SendResult, RelayError> {
    match &target_member.target {
        TargetConfiguration::Acp(_) => Ok(deliver_acp_combined(
            task,
            target_member,
            prompt_batches,
            acp_transport,
        )),
        TargetConfiguration::Tmux(_) => {
            Ok(deliver_one_target_tmux(task, target_member, prompt_batches))
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            Err(super::super::super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            ))
        }
    }
}

/// Delivers a coalesced envelope batch in a single transport call.
///
/// All tasks in `batch` share the same target; the rendered `prompt_batches`
/// represent the combined envelopes. Returns one outcome per task aligned
/// with the slice. Both transports block to a terminal outcome and return it:
/// the relay pre-combines, the transport produces a single outcome, and it is
/// replicated per-task with each task's own `message_id` (for tmux, the success
/// or the reason the K-th paste failed; for ACP, the terminal turn outcome).
///
/// `pre_resolved_pane` lets the worker loop hoist the tmux quiescence wait so
/// post-quiescence task arrivals can be drained into the batch before paste.
/// When `Some` for a tmux target, this skips the in-transport wait + pane
/// resolution and pastes against the supplied pane directly. ACP/UI/Pubsub
/// targets ignore the value.
pub(super) fn deliver_non_ui_target_batch(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    pre_resolved_pane: Option<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Vec<Result<SendResult, RelayError>> {
    debug_assert!(!batch.is_empty());
    match &target_member.target {
        TargetConfiguration::Acp(_) => {
            deliver_acp_batch_via_transport(batch, target_member, prompt_batches, acp_transport)
                .into_iter()
                .map(Ok)
                .collect()
        }
        TargetConfiguration::Tmux(_) => {
            deliver_batch_target_tmux(batch, target_member, prompt_batches, pre_resolved_pane)
                .into_iter()
                .map(Ok)
                .collect()
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            let error = super::super::super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            );
            batch.iter().map(|_| Err(error.clone())).collect()
        }
    }
}

/// Delivers a coalesced ACP batch through the per-target [`AcpTransport`].
///
/// ACP coalescing is relay-side: the worker already combined the batch into a
/// single rendered prompt, so the transport receives one envelope, blocks to
/// terminal, and returns one outcome. That single outcome is replicated across
/// every task in `batch` (each keeps its own `message_id`/`target_session`),
/// matching the tmux fan-out shape.
fn deliver_acp_batch_via_transport(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> Vec<SendResult> {
    let head = &batch[0];
    let outcome = deliver_acp_combined(head, target_member, prompt_batches, acp_transport);
    batch
        .iter()
        .map(|task| SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: outcome.outcome.clone(),
            reason_code: outcome.reason_code.clone(),
            reason: outcome.reason.clone(),
            details: outcome.details.clone(),
        })
        .collect()
}

/// Submits one combined ACP prompt via the transport and converts the single
/// terminal outcome into a [`SendResult`]. Handles the no-transport
/// (bootstrap-failed) and empty-prompt cases the same way the previous in-relay
/// path did.
fn deliver_acp_combined(
    head: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    acp_transport: &mut Option<AcpTransport>,
) -> SendResult {
    let target_session = head.target_session.clone();
    let message_id = head.message_id.clone();

    let Some(transport) = acp_transport.as_mut() else {
        return SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: Some("runtime_acp_worker_unavailable".to_string()),
            reason: Some("ACP worker is unavailable for target session".to_string()),
            details: Some(serde_json::json!({ "target_session": target_member.id })),
        };
    };
    let Some(prompt) = prompt_batches.into_iter().next() else {
        return SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: None,
            reason: Some("ACP delivery received no prompt batch".to_string()),
            details: None,
        };
    };

    let envelope = DeliveryEnvelope {
        message_id: message_id.clone(),
        payload_mode: head.payload_mode,
        rendered: prompt,
        append_enter: head.append_enter,
    };
    let context = DeliveryContext {
        target_session: target_session.clone(),
        runtime_directory: head.runtime_directory.clone(),
        target_member: target_member.clone(),
        pre_resolved_target: None,
        choices_pending_max: head.choices_max_pending,
        choice_decider_sessions: head.choice_decider_sessions.clone(),
    };
    let result = transport.deliver(vec![envelope], &context);
    single_outcome_to_send_result(result, target_session, message_id)
}

fn single_outcome_to_send_result(
    result: DeliveryResult,
    target_session: String,
    message_id: String,
) -> SendResult {
    match result.outcomes.into_iter().next() {
        Some(outcome) => SendResult {
            target_session: outcome.target_session,
            message_id: outcome.message_id,
            outcome: outcome.outcome,
            reason_code: outcome.reason_code,
            reason: outcome.reason,
            details: outcome.details,
        },
        None => SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Failed,
            reason_code: Some("internal_unexpected_failure".to_string()),
            reason: Some("ACP delivery produced no outcome".to_string()),
            details: None,
        },
    }
}

/// Worker-loop entry for the tmux quiescence hoist: waits for the head task's
/// pane to become quiescent and returns the resolved pane target on success.
/// Returns the per-batch failure template on timeout, shutdown, or pane
/// unavailability — the worker fans it out to every task in the coalesced
/// batch. Caller must have established that the head task is envelope-mode
/// and targets a tmux session.
///
/// The poll loop itself lives in [`crate::tmux::transport`]; the relay owns the
/// `QuiescenceOptions` scheduling config and unpacks it into primitives here so
/// the tmux module never depends on relay.
pub(super) fn prepare_tmux_pane_for_envelope_head(
    task: &AsyncDeliveryTask,
    tmux_target: &TmuxTargetConfiguration,
) -> Result<String, Box<SendResult>> {
    debug_assert!(matches!(
        task.payload_mode,
        DeliveryPayloadMode::EnvelopeMessage
    ));
    let tmux_socket_path = crate::runtime::paths::tmux_socket_path_for_runtime_directory(
        task.runtime_directory.as_path(),
    );
    wait_for_quiescent_pane(
        tmux_socket_path.as_path(),
        task.target_session.as_str(),
        task.quiescence.quiet_window,
        task.quiescence.quiescence_timeout,
        tmux_target.prompt_readiness.as_ref(),
    )
    .map_err(|error| Box::new(wait_error_to_send_result(task, error)))
}

/// Single-task tmux delivery (the RawInput path; envelope-mode tasks always go
/// through the hoisted batch path). Routes through [`TmuxTransport`], which
/// resolves the active pane and pastes; `served_successfully` is recorded
/// relay-side here so the transport stays free of relay statics.
fn deliver_one_target_tmux(
    task: &AsyncDeliveryTask,
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
) -> SendResult {
    let envelopes = build_tmux_envelopes(task, prompt_batches);
    let context = tmux_delivery_context(task, target_member, None);
    let result = TmuxTransport::new().deliver(envelopes, &context);
    let send_result =
        single_outcome_to_send_result(result, task.target_session.clone(), task.message_id.clone());
    note_tmux_delivered(task, &send_result);
    send_result
}

/// Tmux delivery for a coalesced envelope batch. The worker proves the pane
/// quiescent once (hoist) and supplies `pre_resolved_pane`; [`TmuxTransport`]
/// pastes every rendered prompt against it and returns one combined outcome,
/// which is fanned out to every task in `batch` using each task's own
/// `message_id`/`target_session`. `served_successfully` is recorded relay-side
/// per delivered task.
fn deliver_batch_target_tmux(
    batch: &[AsyncDeliveryTask],
    target_member: &BundleMember,
    prompt_batches: Vec<String>,
    pre_resolved_pane: Option<String>,
) -> Vec<SendResult> {
    let head = &batch[0];
    let envelopes = build_tmux_envelopes(head, prompt_batches);
    let context = tmux_delivery_context(head, target_member, pre_resolved_pane);
    let result = TmuxTransport::new().deliver(envelopes, &context);
    let template =
        single_outcome_to_send_result(result, head.target_session.clone(), head.message_id.clone());
    let results = replicate_outcome_for_batch(batch, template);
    for (task, send_result) in batch.iter().zip(results.iter()) {
        note_tmux_delivered(task, send_result);
    }
    results
}

/// Records `served_successfully` for a delivered tmux task. Kept relay-side
/// (the ACP path does the equivalent in the worker) so the transport never
/// reaches into relay statics.
fn note_tmux_delivered(task: &AsyncDeliveryTask, send_result: &SendResult) {
    if send_result.outcome == SendOutcome::Delivered {
        let _ = note_session_served_successfully(
            task.runtime_directory.as_path(),
            task.target_session.as_str(),
        );
    }
}

/// Builds the rendered envelopes handed to [`TmuxTransport::deliver`]. Envelope
/// messages paste each coalesced prompt batch (always submitting with Enter);
/// raw input pastes the raw message verbatim, honoring the task's `append_enter`.
fn build_tmux_envelopes(
    head: &AsyncDeliveryTask,
    prompt_batches: Vec<String>,
) -> Vec<DeliveryEnvelope> {
    match head.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => prompt_batches
            .into_iter()
            .map(|rendered| DeliveryEnvelope {
                message_id: head.message_id.clone(),
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                rendered,
                append_enter: true,
            })
            .collect(),
        DeliveryPayloadMode::RawInput => vec![DeliveryEnvelope {
            message_id: head.message_id.clone(),
            payload_mode: DeliveryPayloadMode::RawInput,
            rendered: head.message.clone(),
            append_enter: head.append_enter,
        }],
    }
}

fn tmux_delivery_context(
    head: &AsyncDeliveryTask,
    target_member: &BundleMember,
    pre_resolved_target: Option<String>,
) -> DeliveryContext {
    DeliveryContext {
        target_session: head.target_session.clone(),
        runtime_directory: head.runtime_directory.clone(),
        target_member: target_member.clone(),
        pre_resolved_target,
        choices_pending_max: head.choices_max_pending,
        choice_decider_sessions: head.choice_decider_sessions.clone(),
    }
}

// Reproduces a head-derived SendResult for every task in the batch, swapping
// in each task's own correlation fields. Used to fan one underlying tmux
// delivery outcome out to N coalesced callers, each needing its own result.
fn replicate_outcome_for_batch(
    batch: &[AsyncDeliveryTask],
    template: SendResult,
) -> Vec<SendResult> {
    batch
        .iter()
        .map(|task| SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: template.outcome.clone(),
            reason_code: template.reason_code.clone(),
            reason: template.reason.clone(),
            details: template.details.clone(),
        })
        .collect()
}

/// Maps a tmux quiescence-wait failure to the per-task `SendResult` template the
/// worker fans out across the coalesced batch.
fn wait_error_to_send_result(task: &AsyncDeliveryTask, error: DeliveryWaitError) -> SendResult {
    match error {
        DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        } => {
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
            SendResult {
                target_session: task.target_session.clone(),
                message_id: task.message_id.clone(),
                outcome: SendOutcome::Timeout,
                reason_code: None,
                reason: Some(reason),
                details: None,
            }
        }
        DeliveryWaitError::Failed { reason } => SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: SendOutcome::Failed,
            reason_code: None,
            reason: Some(reason),
            details: None,
        },
        DeliveryWaitError::Shutdown => SendResult {
            target_session: task.target_session.clone(),
            message_id: task.message_id.clone(),
            outcome: SendOutcome::DroppedOnShutdown,
            reason_code: Some(DROPPED_ON_SHUTDOWN_REASON_CODE.to_string()),
            reason: Some(DROPPED_ON_SHUTDOWN_REASON.to_string()),
            details: None,
        },
    }
}
