//! ACP permission handling, decoupled from the relay choice queue.
//!
//! An ACP agent raises a tool-call permission mid-turn. The transport translates
//! that request into a [`ChoiceToMake`], invokes the relay-injected [`Chooser`]
//! (which owns the choice queue and blocks until the operator decides), and maps
//! the returned [`ChoiceMade`] back into the JSON-RPC permission response.
//!
//! The resolver runs on a short-lived thread so the ACP reader thread returns to
//! its `read_line` loop immediately instead of parking on the operator decision.
//! The decision is also recorded into a shared slot the delivery completion path
//! reads, so a cancelled choice can fail the turn with the cancellation taxonomy.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::acp::{PermissionHandler, PermissionRequest};
use crate::transports::{ChoiceMade, ChoiceToMake, Chooser, ThingToChoose};

/// Per-delivery correlation the startup-time [`Chooser`] cannot close over. The
/// transport sources these from the active `DeliveryContext` (and the head
/// envelope's `message_id`) when it builds the [`ChoiceToMake`].
#[derive(Clone, Debug)]
pub(crate) struct ChoiceCorrelation {
    /// The originating send's message id (choice event correlation).
    pub message_id: String,
    /// The target session the choice belongs to.
    pub target_session: String,
    /// Sessions authorized to decide the choice.
    pub decider_sessions: Vec<String>,
}

/// Builds the per-prompt ACP [`PermissionHandler`] that resolves tool-call
/// permissions through the injected [`Chooser`].
///
/// The handler spawns a short-lived resolver thread per request. The thread
/// invokes the (blocking) chooser, records the [`ChoiceMade`] into
/// `pending_choice_outcome` BEFORE writing the JSON-RPC response — the same
/// ordering invariant the previous in-line handler held: the completion path
/// reads the shared slot when building the terminal outcome, and a fast agent
/// reply could otherwise race ahead of the outcome record.
pub(crate) fn build_acp_permission_handler(
    chooser: Chooser,
    correlation: ChoiceCorrelation,
    pending_choice_outcome: Arc<Mutex<Option<ChoiceMade>>>,
) -> PermissionHandler {
    Box::new(
        move |permission_request: PermissionRequest, mut responder| {
            let chooser = Arc::clone(&chooser);
            let correlation = correlation.clone();
            let writer = Arc::clone(&pending_choice_outcome);
            std::thread::Builder::new()
                .name("acp-permission-resolver".to_string())
                .spawn(move || {
                    let choice = build_choice_to_make(&correlation, &permission_request);
                    let made = (chooser)(choice);
                    let response_option_id = match &made {
                        ChoiceMade::Chosen { option_id, .. } => Some(option_id.clone()),
                        ChoiceMade::Cancelled { .. } => None,
                    };
                    *writer.lock().expect("pending_choice_outcome mutex") = Some(made);
                    responder.respond(response_option_id);
                })
                .expect("spawn ACP permission resolver thread");
        },
    )
}

fn build_choice_to_make(
    correlation: &ChoiceCorrelation,
    request: &PermissionRequest,
) -> ChoiceToMake {
    let details = json!({
        "tool_call_title": request.tool_call_title.clone(),
        "options": request.options.clone(),
        "acp_request_id": request.request_id,
        "raw": request.requested_details.clone(),
    });
    ChoiceToMake {
        request_id: request.request_id,
        message_id: correlation.message_id.clone(),
        target_session: correlation.target_session.clone(),
        decider_sessions: correlation.decider_sessions.clone(),
        title: request.tool_call_title.clone(),
        species: request.requested_kind.clone(),
        details,
        options: request
            .options
            .iter()
            .map(|option| ThingToChoose {
                option_id: option.option_id.clone(),
                name: option.name.clone(),
                species: option.kind.clone(),
            })
            .collect(),
    }
}
