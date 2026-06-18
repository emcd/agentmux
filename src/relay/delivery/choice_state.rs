use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::Duration,
};

use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::runtime::{inscriptions::emit_inscription, signals::shutdown_requested};
use crate::transports::{ChoiceMade, ChoiceToMake, Chooser};

use super::super::{
    canonical_session_id,
    stream::{RelayStreamEvent, send_event_to_registered_ui},
};

const CHOICES_QUEUE_UNAVAILABLE_CODE: &str = "runtime_choices_queue_unavailable";

const CHOICE_CANCELLED_CODE: &str = "runtime_choices_request_cancelled";
const CHOICE_INVALIDATED_BY_RESPAWN_CODE: &str = "runtime_choices_request_invalidated_by_respawn";
const CHOICE_ALREADY_RESOLVED_CODE: &str = "runtime_choices_request_already_resolved";
const CHOICES_QUEUE_FULL_CODE: &str = "runtime_choices_queue_full";
const CHOICE_WAIT_POLL_MS: u64 = 100;

type SharedWaiterState = Arc<(Mutex<Option<ChoiceResolutionOutcome>>, Condvar)>;

// The queue is process-local in-memory state keyed by runtime_directory. The
// ACP server is authoritative for whether a choice request is still
// outstanding: a stale on-disk queue from a previous relay run could replay
// records the ACP side has long forgotten, so persistence was removed (relay/43).
#[derive(Clone, Debug, Default)]
struct ChoicesQueueState {
    next_sequence: u64,
    pending: Vec<PendingChoiceRequest>,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PendingChoiceRequest {
    pub(in crate::relay) choice_request_id: String,
    pub(in crate::relay) message_id: String,
    pub(in crate::relay) target_session: String,
    pub(in crate::relay) requested_kind: String,
    pub(in crate::relay) requested_details: Value,
    pub(in crate::relay) enqueued_at: String,
    pub(in crate::relay) sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::relay) enum ChoiceResolutionOutcome {
    Selected {
        option_id: String,
        decided_by: String,
    },
    Cancelled {
        decided_by: String,
        reason_code: String,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct ChoiceEnqueueResult {
    pub(in crate::relay) choice_request_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum ChoiceDecisionKind {
    Selected,
    Cancelled,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct ChoiceDecisionRequest {
    pub(in crate::relay) choice_request_id: String,
    pub(in crate::relay) option_id: Option<String>,
    pub(in crate::relay) decision: ChoiceDecisionKind,
    pub(in crate::relay) decided_by: String,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct ChoiceEventContext {
    pub(in crate::relay) runtime_directory: PathBuf,
    pub(in crate::relay) bundle_name: String,
    pub(in crate::relay) authorized_ui_sessions: Vec<String>,
}

static CHOICES_QUEUES: OnceLock<Mutex<HashMap<PathBuf, ChoicesQueueState>>> = OnceLock::new();
static CHOICE_WAITERS: OnceLock<Mutex<HashMap<String, SharedWaiterState>>> = OnceLock::new();

fn choices_queues() -> &'static Mutex<HashMap<PathBuf, ChoicesQueueState>> {
    CHOICES_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn choice_waiters() -> &'static Mutex<HashMap<String, SharedWaiterState>> {
    CHOICE_WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

// Holds the global queues lock for the duration of `mutate`, so callers can
// read, edit, and re-store one bundle's queue state atomically. Replaces the
// prior file lock + load + store sequence.
fn with_queue_state<R>(
    runtime_directory: &Path,
    mutate: impl FnOnce(&mut ChoicesQueueState) -> R,
) -> Result<R, String> {
    let mut queues = choices_queues()
        .lock()
        .map_err(|_| "failed to lock choices queue state".to_string())?;
    let state = queues
        .entry(runtime_directory.to_path_buf())
        .or_insert_with(|| ChoicesQueueState {
            next_sequence: 1,
            pending: Vec::new(),
        });
    Ok(mutate(state))
}

fn sort_pending_by_sequence(pending: &mut [PendingChoiceRequest]) {
    pending.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then(left.choice_request_id.cmp(&right.choice_request_id))
    });
}

fn pending_choice_option_ids(record: &PendingChoiceRequest) -> Vec<String> {
    record
        .requested_details
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    option
                        .get("option_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Builds the relay-injected [`Chooser`] handed to an ACP transport at startup.
///
/// The transport invokes this on its own thread when an agent raises a tool-call
/// permission; it blocks here on the relay choice queue (via
/// `enqueue_choice_request` and `wait_for_choice_resolution`) until the operator
/// decides, then maps the relay [`ChoiceResolutionOutcome`] into the transport's
/// [`ChoiceMade`]. The closure closes over the target's
/// `bundle_name`/`runtime_directory` and the per-bundle `choices_pending_max`
/// queue bound; only the per-delivery decider list rides in each
/// [`ChoiceToMake`].
pub(in crate::relay) fn build_acp_chooser(
    bundle_name: String,
    runtime_directory: PathBuf,
    choices_pending_max: usize,
) -> Chooser {
    Arc::new(move |choice: ChoiceToMake| -> ChoiceMade {
        let context = ChoiceEventContext {
            runtime_directory: runtime_directory.clone(),
            bundle_name: bundle_name.clone(),
            authorized_ui_sessions: choice.decider_sessions.clone(),
        };
        resolve_choice_via_relay(&context, &choice, choices_pending_max)
    })
}

fn resolve_choice_via_relay(
    context: &ChoiceEventContext,
    choice: &ChoiceToMake,
    choices_pending_max: usize,
) -> ChoiceMade {
    let enqueued = match enqueue_choice_request(
        context,
        choice.message_id.as_str(),
        choice.target_session.as_str(),
        choice.species.as_str(),
        choice.details.clone(),
        choices_pending_max,
    ) {
        Ok(value) => value,
        Err(code) if code == CHOICES_QUEUE_FULL_CODE => {
            return ChoiceMade::Cancelled {
                decided_by: "relay".to_string(),
                reason_code: CHOICES_QUEUE_FULL_CODE.to_string(),
                reason: Some("choices queue is full".to_string()),
            };
        }
        Err(_) => {
            return ChoiceMade::Cancelled {
                decided_by: "relay".to_string(),
                reason_code: CHOICES_QUEUE_UNAVAILABLE_CODE.to_string(),
                reason: Some("failed to enqueue choice request".to_string()),
            };
        }
    };

    let Ok(outcome) = wait_for_choice_resolution(context, enqueued.choice_request_id.as_str())
    else {
        return ChoiceMade::Cancelled {
            decided_by: "relay".to_string(),
            reason_code: CHOICE_CANCELLED_CODE.to_string(),
            reason: Some("failed while waiting for choice decision".to_string()),
        };
    };

    match outcome {
        ChoiceResolutionOutcome::Selected {
            option_id,
            decided_by,
        } => ChoiceMade::Chosen {
            option_id,
            decided_by,
        },
        ChoiceResolutionOutcome::Cancelled {
            decided_by,
            reason_code,
            reason,
        } => ChoiceMade::Cancelled {
            decided_by,
            reason_code,
            reason,
        },
    }
}

pub(in crate::relay) fn enqueue_choice_request(
    context: &ChoiceEventContext,
    message_id: &str,
    target_session: &str,
    requested_kind: &str,
    requested_details: Value,
    pending_max: usize,
) -> Result<ChoiceEnqueueResult, String> {
    let choice_request_id = Uuid::new_v4().to_string();
    let record = PendingChoiceRequest {
        choice_request_id: choice_request_id.clone(),
        message_id: message_id.to_string(),
        target_session: target_session.to_string(),
        requested_kind: requested_kind.to_string(),
        requested_details,
        enqueued_at: timestamp_rfc3339(),
        sequence: 0,
    };
    let stored = with_queue_state(context.runtime_directory.as_path(), |state| {
        if state.pending.len() >= pending_max {
            return Err(CHOICES_QUEUE_FULL_CODE.to_string());
        }
        let mut record = record;
        record.sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.pending.push(record.clone());
        sort_pending_by_sequence(state.pending.as_mut_slice());
        Ok(record)
    })??;
    register_waiter(stored.choice_request_id.as_str())?;
    emit_choices_requested_event(context, &stored);
    super::observability::publish_choices_queue_event(
        context.runtime_directory.as_path(),
        super::observability::ChoicesQueueEvent::Enqueued {
            choice_request_id: stored.choice_request_id.clone(),
            message_id: stored.message_id.clone(),
            target_session: stored.target_session.clone(),
        },
    );
    Ok(ChoiceEnqueueResult { choice_request_id })
}

pub(in crate::relay) fn resolve_choice_request(
    context: &ChoiceEventContext,
    decision: ChoiceDecisionRequest,
) -> Result<ChoiceResolutionOutcome, String> {
    let record = with_queue_state(context.runtime_directory.as_path(), |state| {
        state
            .pending
            .iter()
            .position(|record| record.choice_request_id == decision.choice_request_id)
            .map(|index| state.pending.remove(index))
    })?
    .ok_or_else(|| CHOICE_ALREADY_RESOLVED_CODE.to_string())?;

    let outcome = match decision.decision {
        ChoiceDecisionKind::Selected => {
            let option_id = decision.option_id.ok_or_else(|| {
                "validation_invalid_params: selected outcome requires explicit option_id"
                    .to_string()
            })?;
            let allowed_option_ids = pending_choice_option_ids(&record);
            if !allowed_option_ids
                .iter()
                .any(|candidate| candidate == &option_id)
            {
                return Err(format!(
                    "validation_invalid_params: selected option_id '{}' is not present in pending choice options",
                    option_id
                ));
            }
            ChoiceResolutionOutcome::Selected {
                option_id,
                decided_by: decision.decided_by.clone(),
            }
        }
        ChoiceDecisionKind::Cancelled => ChoiceResolutionOutcome::Cancelled {
            decided_by: decision.decided_by.clone(),
            reason_code: CHOICE_CANCELLED_CODE.to_string(),
            reason: Some("choice request was cancelled by UI decision".to_string()),
        },
    };

    if let Some(waiter) = take_waiter(decision.choice_request_id.as_str())? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_choices_resolved_event(context, &record, &outcome);
    super::observability::publish_choices_queue_event(
        context.runtime_directory.as_path(),
        super::observability::ChoicesQueueEvent::Resolved {
            choice_request_id: record.choice_request_id.clone(),
            target_session: record.target_session.clone(),
        },
    );
    Ok(outcome)
}

pub(in crate::relay) fn wait_for_choice_resolution(
    context: &ChoiceEventContext,
    choice_request_id: &str,
) -> Result<ChoiceResolutionOutcome, String> {
    let waiter =
        get_waiter(choice_request_id)?.ok_or_else(|| CHOICE_ALREADY_RESOLVED_CODE.to_string())?;
    let (lock, condvar) = &*waiter;
    let mut guard = lock
        .lock()
        .map_err(|_| "failed to lock choice waiter".to_string())?;
    loop {
        if let Some(outcome) = guard.clone() {
            return Ok(outcome);
        }
        let wait = condvar
            .wait_timeout(guard, Duration::from_millis(CHOICE_WAIT_POLL_MS))
            .map_err(|_| "failed to wait for choice decision".to_string())?;
        guard = wait.0;
        if shutdown_requested() {
            drop(guard);
            return cancel_choice_request_on_shutdown(context, choice_request_id);
        }
    }
}

pub(in crate::relay) fn emit_choices_snapshot_then_replay(
    context: &ChoiceEventContext,
    ui_session_id: &str,
) -> Result<(), String> {
    let pending = list_pending_choice_requests(context.runtime_directory.as_path())?;
    let snapshot_event = RelayStreamEvent {
        event_type: "choices.snapshot".to_string(),
        target_session: canonical_session_id(ui_session_id, context.bundle_name.as_str()),
        created_at: timestamp_rfc3339(),
        payload: json!({
            "pending_count": pending.len(),
            "choice_request_ids": pending
                .iter()
                .map(|value| value.choice_request_id.clone())
                .collect::<Vec<_>>(),
        }),
    };
    let _ =
        send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &snapshot_event);
    for request in pending {
        let event = choices_requested_event(ui_session_id, &context.bundle_name, &request);
        let _ = send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &event);
    }
    Ok(())
}

// Cancels every pending choice request tied to `target_session` with the
// `runtime_choices_request_invalidated_by_respawn` reason code. Used when
// the ACP worker for that session is being respawned; the dying ACP child can
// no longer accept the operator decision, so the request must be cleared
// before the new child runs `session/load`. Returns the number of records
// invalidated.
pub(in crate::relay) fn invalidate_pending_for_respawn(
    context: &ChoiceEventContext,
    target_session: &str,
) -> Result<usize, String> {
    let invalidated_records: Vec<PendingChoiceRequest> =
        with_queue_state(context.runtime_directory.as_path(), |state| {
            let mut removed = Vec::new();
            state.pending.retain(|record| {
                if record.target_session == target_session {
                    removed.push(record.clone());
                    false
                } else {
                    true
                }
            });
            removed
        })?;
    if invalidated_records.is_empty() {
        return Ok(0);
    }

    for record in &invalidated_records {
        let outcome = ChoiceResolutionOutcome::Cancelled {
            decided_by: "relay".to_string(),
            reason_code: CHOICE_INVALIDATED_BY_RESPAWN_CODE.to_string(),
            reason: Some("ACP worker respawn invalidated the pending choice request".to_string()),
        };
        let had_pending_waiter = match take_waiter(record.choice_request_id.as_str())? {
            Some(waiter) => {
                let (lock, condvar) = &*waiter;
                if let Ok(mut value) = lock.lock() {
                    *value = Some(outcome.clone());
                    condvar.notify_all();
                }
                true
            }
            None => false,
        };
        emit_choices_resolved_event(context, record, &outcome);
        emit_inscription(
            "relay.acp.respawn.choice_invalidated",
            &json!({
                "bundle_name": context.bundle_name,
                "choice_request_id": record.choice_request_id,
                "message_id": record.message_id,
                "target_session": record.target_session,
                "had_pending_waiter": had_pending_waiter,
            }),
        );
        super::observability::publish_choices_queue_event(
            context.runtime_directory.as_path(),
            super::observability::ChoicesQueueEvent::Invalidated {
                choice_request_id: record.choice_request_id.clone(),
                target_session: record.target_session.clone(),
                reason_code: CHOICE_INVALIDATED_BY_RESPAWN_CODE.to_string(),
            },
        );
    }
    Ok(invalidated_records.len())
}

pub(in crate::relay) fn list_pending_choice_requests(
    runtime_directory: &Path,
) -> Result<Vec<PendingChoiceRequest>, String> {
    with_queue_state(runtime_directory, |state| state.pending.clone())
}

// Pre-seeds the in-memory choices queue for a runtime directory. Exposed
// for integration tests that need to assert on UI snapshot/replay behavior
// for deterministically named choice requests; production code paths use
// `enqueue_choice_request` instead.
pub fn install_pending_choice_request_for_testing(
    runtime_directory: &Path,
    choice_request_id: &str,
    message_id: &str,
    target_session: &str,
    requested_kind: &str,
    requested_details: Value,
) -> Result<(), String> {
    with_queue_state(runtime_directory, |state| {
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.pending.push(PendingChoiceRequest {
            choice_request_id: choice_request_id.to_string(),
            message_id: message_id.to_string(),
            target_session: target_session.to_string(),
            requested_kind: requested_kind.to_string(),
            requested_details,
            enqueued_at: timestamp_rfc3339(),
            sequence,
        });
        sort_pending_by_sequence(state.pending.as_mut_slice());
    })
}

fn cancel_choice_request_on_shutdown(
    context: &ChoiceEventContext,
    choice_request_id: &str,
) -> Result<ChoiceResolutionOutcome, String> {
    let record = with_queue_state(context.runtime_directory.as_path(), |state| {
        state
            .pending
            .iter()
            .position(|record| record.choice_request_id == choice_request_id)
            .map(|index| state.pending.remove(index))
    })?;
    let Some(record) = record else {
        return Ok(ChoiceResolutionOutcome::Cancelled {
            decided_by: "relay".to_string(),
            reason_code: CHOICE_ALREADY_RESOLVED_CODE.to_string(),
            reason: Some("choice request was already resolved".to_string()),
        });
    };
    let outcome = ChoiceResolutionOutcome::Cancelled {
        decided_by: "relay".to_string(),
        reason_code: CHOICE_CANCELLED_CODE.to_string(),
        reason: Some("relay shutdown cancelled pending choice request".to_string()),
    };
    if let Some(waiter) = take_waiter(choice_request_id)? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_choices_resolved_event(context, &record, &outcome);
    super::observability::publish_choices_queue_event(
        context.runtime_directory.as_path(),
        super::observability::ChoicesQueueEvent::Invalidated {
            choice_request_id: record.choice_request_id.clone(),
            target_session: record.target_session.clone(),
            reason_code: CHOICE_CANCELLED_CODE.to_string(),
        },
    );
    Ok(outcome)
}

fn emit_choices_requested_event(context: &ChoiceEventContext, request: &PendingChoiceRequest) {
    for ui_session_id in &context.authorized_ui_sessions {
        let event = choices_requested_event(ui_session_id.as_str(), &context.bundle_name, request);
        let _ = send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &event);
    }
    emit_inscription(
        "relay.choices.requested",
        &json!({
            "bundle_name": context.bundle_name,
            "choice_request_id": request.choice_request_id,
            "message_id": request.message_id,
            "target_session": request.target_session,
            "requested_kind": request.requested_kind,
            "enqueued_at": request.enqueued_at,
        }),
    );
}

fn emit_choices_resolved_event(
    context: &ChoiceEventContext,
    request: &PendingChoiceRequest,
    outcome: &ChoiceResolutionOutcome,
) {
    let (outcome_label, reason_code, decided_by, reason) = match outcome {
        ChoiceResolutionOutcome::Selected { decided_by, .. } => (
            "selected",
            Value::Null,
            Value::String(decided_by.clone()),
            Value::Null,
        ),
        ChoiceResolutionOutcome::Cancelled {
            decided_by,
            reason_code,
            reason,
        } => (
            "cancelled",
            Value::String(reason_code.clone()),
            Value::String(decided_by.clone()),
            reason.clone().map(Value::String).unwrap_or(Value::Null),
        ),
    };
    for ui_session_id in &context.authorized_ui_sessions {
        let event = RelayStreamEvent {
            event_type: "choices.resolved".to_string(),
            target_session: canonical_session_id(ui_session_id, context.bundle_name.as_str()),
            created_at: timestamp_rfc3339(),
            payload: json!({
                "message_id": request.message_id,
                "choice_request_id": request.choice_request_id,
                "outcome": outcome_label,
                "reason_code": reason_code,
                "decided_by": decided_by,
                "reason": reason,
                "resolved_at": timestamp_rfc3339(),
            }),
        };
        let _ = send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &event);
    }
    emit_inscription(
        "relay.choices.resolved",
        &json!({
            "bundle_name": context.bundle_name,
            "choice_request_id": request.choice_request_id,
            "message_id": request.message_id,
            "outcome": outcome_label,
        }),
    );
}

fn choices_requested_event(
    ui_session_id: &str,
    bundle_name: &str,
    request: &PendingChoiceRequest,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: "choices.requested".to_string(),
        target_session: canonical_session_id(ui_session_id, bundle_name),
        created_at: timestamp_rfc3339(),
        payload: json!({
            "message_id": request.message_id,
            "choice_request_id": request.choice_request_id,
            "target_session": canonical_session_id(request.target_session.as_str(), bundle_name),
            "requested_kind": request.requested_kind,
            "requested_details": request.requested_details,
            "enqueued_at": request.enqueued_at,
        }),
    }
}

fn register_waiter(choice_request_id: &str) -> Result<(), String> {
    let mut waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    waiters.insert(
        choice_request_id.to_string(),
        Arc::new((Mutex::new(None), Condvar::new())),
    );
    Ok(())
}

fn get_waiter(choice_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    Ok(waiters.get(choice_request_id).cloned())
}

fn take_waiter(choice_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let mut waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    Ok(waiters.remove(choice_request_id))
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
