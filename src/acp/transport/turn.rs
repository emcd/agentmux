//! Turn submission and observability for the ACP transport.

use std::sync::{Arc, Mutex};

use crate::acp::client::AcpStdioClient;
use crate::acp::permission::{ChoiceCorrelation, build_acp_permission_handler};
use crate::acp::{
    DispatchHandler, PermissionHandler, PermissionResponder, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};
use crate::runtime::signals::shutdown_requested;
use crate::transports::{ChoiceMade, SubmissionEvidence, WorkerReadinessState};

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

/// Submits one combined prompt as an ACP turn and reports what the submission
/// proved for the unit it carried.
///
/// The framed `session/prompt` write is the delivery boundary: `Submitted` is
/// returned immediately after the write succeeds, before replay-buffer locks or
/// `on_dispatched` run. The turn's later completion, permission requests, or
/// connection close are target-health observability — they drive readiness and
/// the respawn signal, never a second delivery outcome for an already-resolved
/// member. Active-prompt refusal and serialization failure map to
/// `NotSubmitted`; a stdin write or flush error without proof that zero bytes
/// left maps to `SubmissionUnknown`. No elapsed-time path bounds the wait on the
/// ACP side; the relay's submission-timeout watchdog bounds the supervised
/// code's runtime instead, which it can do precisely because this function
/// returns at the write.
///
/// One value for the whole unit, and that is the point rather than a
/// simplification: one framed write carried every member of it, so one result is
/// what actually happened to all of them. Deriving a value per member would make
/// disagreement between siblings representable when nothing could produce it.
///
/// **The turn is not observed here**, and that ordering is load-bearing. The
/// member's evidence is settled at the write, before the replay-buffer locks and
/// before anything waits on the agent; observing the turn from inside this call
/// would put an unbounded wait — and a panic site — between a write that
/// succeeded and the relay learning it did. What comes back instead is a
/// [`TurnObservation`] the caller drives before it writes again, which is also
/// where the wait belongs: the turn completing is what makes the worker ready
/// for the next one.
pub(crate) fn submit_turn(
    client: &mut AcpStdioClient,
    ctx: &TurnContext,
    respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    prompt: &str,
    head_message_id: &str,
    decider_sessions: &[String],
) -> (SubmissionEvidence, Option<TurnObservation>) {
    let pending_choice: Arc<Mutex<Option<ChoiceMade>>> = Arc::new(Mutex::new(None));
    let completion_slot: Arc<Mutex<Option<PromptCompletion>>> = Arc::new(Mutex::new(None));

    let head_message_id = head_message_id.to_string();

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
            // Replay-buffer locks + on_dispatched (Busy) follow the write; the
            // turn lifecycle is observability only. The evidence returned below
            // is settled before either runs, so neither blocking nor panicking
            // here can change what the unit reports.
            client.note_prompt_dispatched(prompt, Some(on_dispatched));
            (
                SubmissionEvidence::Submitted,
                Some(TurnObservation {
                    completion_slot,
                    pending_choice,
                }),
            )
        }
        PromptDispatchOutcome::TransportUnavailable { reason: _ } => {
            // A stdin write or flush error without proof that zero bytes left
            // cannot assert non-delivery.
            set_turn_readiness(ctx, WorkerReadinessState::Unavailable);
            raise_respawn_signal(respawn_needed_tx);
            (SubmissionEvidence::SubmissionUnknown, None)
        }
        PromptDispatchOutcome::SerializationFailed(_) => {
            // Active-prompt refusal and serialization failure are positive
            // non-delivery: nothing was written. The transport is healthy, so
            // readiness stays as it was, and there is no turn to observe.
            (SubmissionEvidence::NotSubmitted, None)
        }
    }
}

/// A dispatched turn whose lifecycle has yet to be observed.
///
/// Carries only what the observation reads. Its members are already resolved, so
/// this decides nothing about them — what it settles is when the worker becomes
/// ready again, and whether a respawn is called for.
pub(crate) struct TurnObservation {
    completion_slot: Arc<Mutex<Option<PromptCompletion>>>,
    pending_choice: Arc<Mutex<Option<ChoiceMade>>>,
}

impl TurnObservation {
    /// Waits for the turn to settle and publishes what that settling means.
    pub(crate) fn observe(
        self,
        client: &mut AcpStdioClient,
        ctx: &TurnContext,
        respawn_needed_tx: &tokio::sync::watch::Sender<u64>,
    ) {
        observe_acp_turn(
            client,
            ctx,
            respawn_needed_tx,
            &self.completion_slot,
            &self.pending_choice,
        );
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
