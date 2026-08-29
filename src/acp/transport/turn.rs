//! Turn submission and observability for the ACP transport.

use std::sync::{Arc, Mutex};

use crate::acp::client::AcpStdioClient;
use crate::acp::permission::{ChoiceCorrelation, build_acp_permission_handler};
use crate::acp::{
    DispatchHandler, PermissionHandler, PermissionResponder, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};
use crate::runtime::signals::shutdown_requested;
use crate::transports::{
    ChoiceMade, SendOutcome, SingleDeliveryOutcome, SubmissionEvidence, WorkerReadinessState,
};

use super::state::{ACP_PROMPT_WAIT_POLL_INTERVAL, AcpSharedState, raise_respawn_signal};

/// ACP runtime state shared across turn submission functions.
pub(crate) struct TurnContext<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) shared: &'a Arc<AcpSharedState>,
    pub(crate) chooser: &'a Option<crate::transports::Chooser>,
    pub(crate) target_session: &'a str,
}

/// Sets the transport-internal readiness and mirrors the transition to the relay
/// global registry when a mirror is installed. Centralizes the per-turn readiness
/// transitions inside the delivery task so the relay worker no longer drives
/// `mark_busy` / `mirror_settled_readiness`.
pub(crate) fn set_turn_readiness(ctx: &TurnContext, state: WorkerReadinessState) {
    set_shared_readiness(ctx.shared, state);
}

/// Writes `state` to the shared readiness slot and mirrors it to the relay global
/// registry when a mirror is installed. Shared by [`set_turn_readiness`] and the
/// `on_dispatched` Busy transition (which holds the `Arc` directly).
pub(crate) fn set_shared_readiness(shared: &AcpSharedState, state: WorkerReadinessState) {
    *shared.readiness.lock().expect("readiness mutex") = state;
    if let Some(mirror) = shared.mirror_state.as_ref() {
        mirror(state);
    }
}

/// Who declared the packing unit a turn is about to write.
///
/// Not a bare `Option<&dyn PartitionSink>`, because the distinction is about
/// ownership rather than availability: raw's unit is declared by the relay and
/// declaring it again here would be refused as a second binding. The reason this
/// layer cannot declare raw's is that it cannot name the member — `submit_raw_turn`
/// reaches this function with a synthetic empty message id, since neither
/// `Transport::raww` nor the write channel carries the real one.
#[derive(Clone, Copy)]
pub(crate) enum TurnUnit {
    /// An envelope group: declare it here, from the members about to be written.
    DeclareHere,
    /// A raw write: the relay already declared its singleton unit and records its
    /// evidence through the member-keyed ledger entry point.
    RelayDeclared,
    /// A terminal-outcome receipt: no unit exists for it anywhere, because it
    /// bypassed admission and holds no ledger entry.
    ///
    /// Distinct from [`RelayDeclared`](Self::RelayDeclared) even though both
    /// decline to declare here, because the reason is different and the
    /// difference is load-bearing: raw *is* bound and resolves through the guard,
    /// while a receipt is bound to nothing and resolves only through its own
    /// outcome sender. Declaring one would be refused — the ledger cannot tell a
    /// member it never had from one that already terminalized — and the refusal
    /// would silently drop a receipt the relay committed to sending.
    Untracked,
}

/// Submits one combined prompt as an ACP turn and resolves every member of the
/// group from the submission evidence.
///
/// The framed `session/prompt` write is the delivery boundary: `Submitted`
/// (member resolves `Delivered`) is recorded immediately after the write
/// succeeds, before replay-buffer locks or `on_dispatched` run. The turn's
/// later completion, permission requests, or connection close are target-health
/// observability — they drive readiness and the respawn signal, never a second
/// delivery outcome for an already-resolved member. Active-prompt refusal and
/// serialization failure map to `not_submitted`; a stdin write or flush error
/// without proof that zero bytes left maps to `submission_unknown`. No
/// elapsed-time path bounds the wait on the ACP side; the relay's
/// submission-timeout watchdog bounds the supervised code's runtime instead,
/// which it can do precisely because this function resolves at the write.
pub(crate) fn submit_envelope_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    prompt: &str,
    members: Vec<(String, tokio::sync::oneshot::Sender<SingleDeliveryOutcome>)>,
    decider_sessions: &[String],
    unit: TurnUnit,
) {
    // Declared before the framed write below, from the members this turn's one
    // `session/prompt` will carry. After the write, partial effect cannot be
    // excluded for any of them.
    let declared = match unit {
        TurnUnit::DeclareHere => {
            let member_ids: Vec<&str> = members
                .iter()
                .map(|(message_id, _)| message_id.as_str())
                .collect();
            match ctx.shared.partition_sink.declare(&member_ids) {
                Ok(unit) => Some(unit),
                Err(_) => {
                    // The relay refused the whole proposed unit, so this turn
                    // must produce no effect. Dropping the senders unresolved
                    // hands the members back to the guard, which derives
                    // `not_submitted` from their being unbound; sending an
                    // outcome here would be a second resolution for a member the
                    // relay may already have resolved.
                    drop(members);
                    return;
                }
            }
        }
        TurnUnit::RelayDeclared | TurnUnit::Untracked => None,
    };
    let record = |evidence: SubmissionEvidence| {
        if let Some(unit) = declared {
            ctx.shared.partition_sink.record(unit, evidence);
        }
    };
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

    let head_message_id = members
        .first()
        .map(|(message_id, _)| message_id.clone())
        .unwrap_or_default();

    let shared_for_dispatch = Arc::clone(ctx.shared);
    let on_dispatched: DispatchHandler = Box::new(move || {
        set_shared_readiness(&shared_for_dispatch, WorkerReadinessState::Busy);
    });

    let on_permission = if let Some(chooser) = ctx.chooser {
        let correlation = ChoiceCorrelation {
            message_id: head_message_id,
            target_session: ctx.target_session.to_string(),
            decider_sessions: decider_sessions.to_vec(),
        };
        let shared_for_executors = Arc::clone(ctx.shared);
        let mut inner = build_acp_permission_handler(
            chooser.clone(),
            correlation,
            Arc::clone(&pending_choice),
            Arc::new(move |handle| shared_for_executors.note_permission_executor(handle)),
        );
        let wrapped: PermissionHandler = Box::new(move |req, responder| {
            (inner)(req, responder);
        });
        wrapped
    } else {
        let wrapped: PermissionHandler = Box::new(|_req, mut responder: PermissionResponder| {
            responder.respond(None);
        });
        wrapped
    };

    let completion_writer = Arc::clone(&completion_slot);
    let on_completion: PromptCompletionHandler = Box::new(move |completion| {
        *completion_writer.lock().expect("completion slot mutex") = Some(completion);
    });

    // `Submitted` is returned immediately after the framed write succeeds,
    // before replay-buffer locks or `on_dispatched` — so the member evidence is
    // recorded (resolved below) before either can block or panic.
    let dispatch = client.prompt(ctx.session_id, prompt, Some(on_permission), on_completion);

    match dispatch {
        PromptDispatchOutcome::Submitted => {
            // The framed write succeeded: every member of this group resolves
            // `Delivered` at the write, before the replay-buffer locks or
            // `on_dispatched` below. The unit's record is written first, so a
            // member this fan-out never reaches still resolves from what the
            // write proved rather than from its own absence.
            record(SubmissionEvidence::Submitted);
            for (message_id, sender) in members {
                let _ = sender.send(delivered_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                ));
            }
            // Replay-buffer locks + on_dispatched (Busy) follow the evidence
            // recording; the turn lifecycle is observability only.
            client.note_prompt_dispatched(prompt, Some(on_dispatched));
            observe_acp_turn(
                client,
                ctx,
                respawn_needed_tx,
                &completion_slot,
                &pending_choice,
            );
        }
        PromptDispatchOutcome::TransportUnavailable { reason } => {
            // A stdin write or flush error without proof that zero bytes left
            // cannot assert non-delivery: the member resolves submission_unknown.
            set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
            raise_respawn_signal(respawn_needed_tx);
            record(SubmissionEvidence::SubmissionUnknown);
            for (message_id, sender) in members {
                let _ = sender.send(submission_unknown_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                    &reason,
                ));
            }
        }
        PromptDispatchOutcome::SerializationFailed(reason) => {
            // Active-prompt refusal and serialization failure are positive
            // non-delivery: nothing was written, so the member resolves
            // not_submitted. The transport is healthy, so readiness stays as-is.
            record(SubmissionEvidence::NotSubmitted);
            for (message_id, sender) in members {
                let _ = sender.send(not_submitted_outcome(
                    ctx.target_session.to_string(),
                    message_id,
                    &reason,
                ));
            }
        }
    }
}

/// Observes the turn lifecycle after a `Submitted` framed write. The member
/// already resolved `Delivered` at the write, so this wait drives readiness and
/// the respawn signal only: a normal completion returns the worker to
/// `Available`, a connection close marks it `Unavailable` and raises the respawn
/// signal, and an abandoned wait (shutdown) leaves the worker draining.
pub(crate) fn observe_acp_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    completion_slot: &Arc<Mutex<Option<PromptCompletion>>>,
    pending_choice: &Arc<Mutex<Option<ChoiceMade>>>,
) {
    // No elapsed-time path bounds this wait, and the relay's submission-timeout
    // watchdog does not reach it either: every member of the turn resolved at the
    // framed write, so nothing is in flight for the bound to be anchored against.
    // That is the intended shape — this wait is over the agent's inference, which
    // is exactly what the watchdog is not allowed to measure. Returns true if the
    // prompt completed, false if shutdown was requested.
    let completed = loop {
        if client.wait_for_prompt_complete(ACP_PROMPT_WAIT_POLL_INTERVAL) {
            break true;
        }
        if shutdown_requested() {
            break false;
        }
    };
    if !completed {
        let _ = completion_slot
            .lock()
            .expect("completion slot mutex")
            .take();
        let _ = pending_choice.lock().expect("pending_choice mutex").take();
        set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
        return;
    }
    let completion = completion_slot
        .lock()
        .expect("completion slot mutex")
        .take();
    let pending = pending_choice.lock().expect("pending_choice mutex").take();
    let (final_state, requires_respawn) = build_acp_turn_observability(completion, pending);
    set_turn_readiness(ctx, final_state);
    if requires_respawn {
        raise_respawn_signal(respawn_needed_tx);
    }
}

/// Submits raw content as an ACP turn (no envelope framing). The framed write
/// is the delivery boundary exactly as in the envelope path; no elapsed-time
/// bound is applied here either.
pub(crate) fn submit_raw_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    content: &str,
    _append_enter: bool,
    outcome_tx: tokio::sync::oneshot::Sender<SingleDeliveryOutcome>,
) {
    submit_envelope_turn(
        client,
        ctx,
        respawn_needed_tx,
        content,
        vec![(String::new(), outcome_tx)],
        &[],
        TurnUnit::RelayDeclared,
    );
}

pub(crate) fn delivered_outcome(
    target_session: String,
    message_id: String,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::Delivered,
        reason_code: None,
        reason: None,
        details: None,
    }
}

pub(crate) fn not_submitted_outcome(
    target_session: String,
    message_id: String,
    reason: &str,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::NotSubmitted,
        reason_code: Some("not_submitted".to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}

pub(crate) fn submission_unknown_outcome(
    target_session: String,
    message_id: String,
    reason: &str,
) -> SingleDeliveryOutcome {
    SingleDeliveryOutcome {
        target_session,
        message_id,
        outcome: SendOutcome::SubmissionUnknown,
        reason_code: Some("submission_unknown".to_string()),
        reason: Some(reason.to_string()),
        details: None,
    }
}

/// Classifies a settled turn's lifecycle for target-health observability.
///
/// The member already resolved `Delivered` at the framed write, so the turn's
/// completion and any operator-choice outcome never produce a second delivery
/// outcome — they only drive the worker's readiness and the respawn signal. A
/// normal completion returns the worker to `Available`; a connection close
/// marks it `Unavailable` and requests a respawn; an abandoned wait (shutdown)
/// leaves the worker draining. `ProtocolError` and an unsupported stop reason
/// keep the worker `Available`, because the agent is still responsive and a
/// bad turn is not a recoverable failure.
pub(crate) fn build_acp_turn_observability(
    completion: Option<PromptCompletion>,
    pending_choice_outcome: Option<ChoiceMade>,
) -> (WorkerReadinessState, bool) {
    if let Some(ChoiceMade::Cancelled { .. }) = pending_choice_outcome {
        return (WorkerReadinessState::Available, false);
    }

    let Some(completion) = completion else {
        // No completion observed before the wait was abandoned: shutdown. The
        // member already resolved `Delivered` at the write; the worker is
        // draining rather than accepting more turns.
        return (WorkerReadinessState::Unavailable, false);
    };

    match completion {
        PromptCompletion::Completed { stop_reason } => match stop_reason.as_str() {
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => {
                (WorkerReadinessState::Available, false)
            }
            "cancelled" => (WorkerReadinessState::Available, false),
            _ => (WorkerReadinessState::Available, false),
        },
        PromptCompletion::ProtocolError(_) => (WorkerReadinessState::Available, false),
        PromptCompletion::ConnectionClosed { .. } => (WorkerReadinessState::Unavailable, true),
    }
}
