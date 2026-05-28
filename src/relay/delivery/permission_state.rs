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

use super::super::{
    canonical_session_id,
    stream::{RelayStreamEvent, send_event_to_registered_ui},
};

const PERMISSION_CANCELLED_CODE: &str = "runtime_permission_request_cancelled";
const PERMISSION_INVALIDATED_BY_RESPAWN_CODE: &str =
    "runtime_permission_request_invalidated_by_respawn";
const PERMISSION_ALREADY_RESOLVED_CODE: &str = "runtime_permission_request_already_resolved";
const PERMISSION_QUEUE_FULL_CODE: &str = "runtime_permission_queue_full";
const PERMISSION_WAIT_POLL_MS: u64 = 100;

type SharedWaiterState = Arc<(Mutex<Option<PermissionResolutionOutcome>>, Condvar)>;

// The queue is process-local in-memory state keyed by runtime_directory. The
// ACP server is authoritative for whether a permission request is still
// outstanding: a stale on-disk queue from a previous relay run could replay
// records the ACP side has long forgotten, so persistence was removed (relay/43).
#[derive(Clone, Debug, Default)]
struct PermissionQueueState {
    next_sequence: u64,
    pending: Vec<PendingPermissionRequest>,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PendingPermissionRequest {
    pub(in crate::relay) permission_request_id: String,
    pub(in crate::relay) message_id: String,
    pub(in crate::relay) target_session: String,
    pub(in crate::relay) requested_kind: String,
    pub(in crate::relay) requested_details: Value,
    pub(in crate::relay) enqueued_at: String,
    pub(in crate::relay) sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::relay) enum PermissionResolutionOutcome {
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
pub(in crate::relay) struct PermissionEnqueueResult {
    pub(in crate::relay) permission_request_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum PermissionDecisionKind {
    Selected,
    Cancelled,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PermissionDecisionRequest {
    pub(in crate::relay) permission_request_id: String,
    pub(in crate::relay) option_id: Option<String>,
    pub(in crate::relay) decision: PermissionDecisionKind,
    pub(in crate::relay) decided_by: String,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PermissionEventContext {
    pub(in crate::relay) runtime_directory: PathBuf,
    pub(in crate::relay) bundle_name: String,
    pub(in crate::relay) authorized_ui_sessions: Vec<String>,
}

static PERMISSION_QUEUES: OnceLock<Mutex<HashMap<PathBuf, PermissionQueueState>>> = OnceLock::new();
static PERMISSION_WAITERS: OnceLock<Mutex<HashMap<String, SharedWaiterState>>> = OnceLock::new();

fn permission_queues() -> &'static Mutex<HashMap<PathBuf, PermissionQueueState>> {
    PERMISSION_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn permission_waiters() -> &'static Mutex<HashMap<String, SharedWaiterState>> {
    PERMISSION_WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

// Holds the global queues lock for the duration of `mutate`, so callers can
// read, edit, and re-store one bundle's queue state atomically. Replaces the
// prior file lock + load + store sequence.
fn with_queue_state<R>(
    runtime_directory: &Path,
    mutate: impl FnOnce(&mut PermissionQueueState) -> R,
) -> Result<R, String> {
    let mut queues = permission_queues()
        .lock()
        .map_err(|_| "failed to lock permission queue state".to_string())?;
    let state = queues
        .entry(runtime_directory.to_path_buf())
        .or_insert_with(|| PermissionQueueState {
            next_sequence: 1,
            pending: Vec::new(),
        });
    Ok(mutate(state))
}

fn sort_pending_by_sequence(pending: &mut [PendingPermissionRequest]) {
    pending.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then(left.permission_request_id.cmp(&right.permission_request_id))
    });
}

fn pending_permission_option_ids(record: &PendingPermissionRequest) -> Vec<String> {
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

pub(in crate::relay) fn enqueue_permission_request(
    context: &PermissionEventContext,
    message_id: &str,
    target_session: &str,
    requested_kind: &str,
    requested_details: Value,
    max_pending: usize,
) -> Result<PermissionEnqueueResult, String> {
    let permission_request_id = Uuid::new_v4().to_string();
    let record = PendingPermissionRequest {
        permission_request_id: permission_request_id.clone(),
        message_id: message_id.to_string(),
        target_session: target_session.to_string(),
        requested_kind: requested_kind.to_string(),
        requested_details,
        enqueued_at: timestamp_rfc3339(),
        sequence: 0,
    };
    let stored = with_queue_state(context.runtime_directory.as_path(), |state| {
        if state.pending.len() >= max_pending {
            return Err(PERMISSION_QUEUE_FULL_CODE.to_string());
        }
        let mut record = record;
        record.sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.pending.push(record.clone());
        sort_pending_by_sequence(state.pending.as_mut_slice());
        Ok(record)
    })??;
    register_waiter(stored.permission_request_id.as_str())?;
    emit_permission_requested_event(context, &stored);
    super::observability::publish_permission_queue_event(
        context.runtime_directory.as_path(),
        super::observability::PermissionQueueEvent::Enqueued {
            permission_request_id: stored.permission_request_id.clone(),
            message_id: stored.message_id.clone(),
            target_session: stored.target_session.clone(),
        },
    );
    Ok(PermissionEnqueueResult {
        permission_request_id,
    })
}

pub(in crate::relay) fn resolve_permission_request(
    context: &PermissionEventContext,
    decision: PermissionDecisionRequest,
) -> Result<PermissionResolutionOutcome, String> {
    let record = with_queue_state(context.runtime_directory.as_path(), |state| {
        state
            .pending
            .iter()
            .position(|record| record.permission_request_id == decision.permission_request_id)
            .map(|index| state.pending.remove(index))
    })?
    .ok_or_else(|| PERMISSION_ALREADY_RESOLVED_CODE.to_string())?;

    let outcome = match decision.decision {
        PermissionDecisionKind::Selected => {
            let option_id = decision.option_id.ok_or_else(|| {
                "validation_invalid_params: selected outcome requires explicit option_id"
                    .to_string()
            })?;
            let allowed_option_ids = pending_permission_option_ids(&record);
            if !allowed_option_ids
                .iter()
                .any(|candidate| candidate == &option_id)
            {
                return Err(format!(
                    "validation_invalid_params: selected option_id '{}' is not present in pending permission options",
                    option_id
                ));
            }
            PermissionResolutionOutcome::Selected {
                option_id,
                decided_by: decision.decided_by.clone(),
            }
        }
        PermissionDecisionKind::Cancelled => PermissionResolutionOutcome::Cancelled {
            decided_by: decision.decided_by.clone(),
            reason_code: PERMISSION_CANCELLED_CODE.to_string(),
            reason: Some("permission request was cancelled by UI decision".to_string()),
        },
    };

    if let Some(waiter) = take_waiter(decision.permission_request_id.as_str())? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_permission_resolved_event(context, &record, &outcome);
    super::observability::publish_permission_queue_event(
        context.runtime_directory.as_path(),
        super::observability::PermissionQueueEvent::Resolved {
            permission_request_id: record.permission_request_id.clone(),
            target_session: record.target_session.clone(),
        },
    );
    Ok(outcome)
}

pub(in crate::relay) fn wait_for_permission_resolution(
    context: &PermissionEventContext,
    permission_request_id: &str,
) -> Result<PermissionResolutionOutcome, String> {
    let waiter = get_waiter(permission_request_id)?
        .ok_or_else(|| PERMISSION_ALREADY_RESOLVED_CODE.to_string())?;
    let (lock, condvar) = &*waiter;
    let mut guard = lock
        .lock()
        .map_err(|_| "failed to lock permission waiter".to_string())?;
    loop {
        if let Some(outcome) = guard.clone() {
            return Ok(outcome);
        }
        let wait = condvar
            .wait_timeout(guard, Duration::from_millis(PERMISSION_WAIT_POLL_MS))
            .map_err(|_| "failed to wait for permission decision".to_string())?;
        guard = wait.0;
        if shutdown_requested() {
            drop(guard);
            return cancel_permission_request_on_shutdown(context, permission_request_id);
        }
    }
}

pub(in crate::relay) fn emit_permission_snapshot_then_replay(
    context: &PermissionEventContext,
    ui_session_id: &str,
) -> Result<(), String> {
    let pending = list_pending_permission_requests(context.runtime_directory.as_path())?;
    let snapshot_event = RelayStreamEvent {
        event_type: "permission.snapshot".to_string(),
        bundle_name: context.bundle_name.clone(),
        target_session: ui_session_id.to_string(),
        created_at: timestamp_rfc3339(),
        payload: json!({
            "pending_count": pending.len(),
            "permission_request_ids": pending
                .iter()
                .map(|value| value.permission_request_id.clone())
                .collect::<Vec<_>>(),
        }),
    };
    let _ =
        send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &snapshot_event);
    for request in pending {
        let event = permission_requested_event(ui_session_id, &context.bundle_name, &request);
        let _ = send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &event);
    }
    Ok(())
}

// Cancels every pending permission request tied to `target_session` with the
// `runtime_permission_request_invalidated_by_respawn` reason code. Used when
// the ACP worker for that session is being respawned; the dying ACP child can
// no longer accept the operator decision, so the request must be cleared
// before the new child runs `session/load`. Returns the number of records
// invalidated.
pub(in crate::relay) fn invalidate_pending_for_respawn(
    context: &PermissionEventContext,
    target_session: &str,
) -> Result<usize, String> {
    let invalidated_records: Vec<PendingPermissionRequest> =
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
        let outcome = PermissionResolutionOutcome::Cancelled {
            decided_by: "relay".to_string(),
            reason_code: PERMISSION_INVALIDATED_BY_RESPAWN_CODE.to_string(),
            reason: Some(
                "ACP worker respawn invalidated the pending permission request".to_string(),
            ),
        };
        let had_pending_waiter = match take_waiter(record.permission_request_id.as_str())? {
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
        emit_permission_resolved_event(context, record, &outcome);
        emit_inscription(
            "relay.acp.respawn.permission_invalidated",
            &json!({
                "bundle_name": context.bundle_name,
                "permission_request_id": record.permission_request_id,
                "message_id": record.message_id,
                "target_session": record.target_session,
                "had_pending_waiter": had_pending_waiter,
            }),
        );
        super::observability::publish_permission_queue_event(
            context.runtime_directory.as_path(),
            super::observability::PermissionQueueEvent::Invalidated {
                permission_request_id: record.permission_request_id.clone(),
                target_session: record.target_session.clone(),
                reason_code: PERMISSION_INVALIDATED_BY_RESPAWN_CODE.to_string(),
            },
        );
    }
    Ok(invalidated_records.len())
}

pub(in crate::relay) fn list_pending_permission_requests(
    runtime_directory: &Path,
) -> Result<Vec<PendingPermissionRequest>, String> {
    with_queue_state(runtime_directory, |state| state.pending.clone())
}

// Pre-seeds the in-memory permission queue for a runtime directory. Exposed
// for integration tests that need to assert on UI snapshot/replay behavior
// for deterministically named permission requests; production code paths use
// `enqueue_permission_request` instead.
pub fn install_pending_permission_request_for_testing(
    runtime_directory: &Path,
    permission_request_id: &str,
    message_id: &str,
    target_session: &str,
    requested_kind: &str,
    requested_details: Value,
) -> Result<(), String> {
    with_queue_state(runtime_directory, |state| {
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.pending.push(PendingPermissionRequest {
            permission_request_id: permission_request_id.to_string(),
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

fn cancel_permission_request_on_shutdown(
    context: &PermissionEventContext,
    permission_request_id: &str,
) -> Result<PermissionResolutionOutcome, String> {
    let record = with_queue_state(context.runtime_directory.as_path(), |state| {
        state
            .pending
            .iter()
            .position(|record| record.permission_request_id == permission_request_id)
            .map(|index| state.pending.remove(index))
    })?;
    let Some(record) = record else {
        return Ok(PermissionResolutionOutcome::Cancelled {
            decided_by: "relay".to_string(),
            reason_code: PERMISSION_ALREADY_RESOLVED_CODE.to_string(),
            reason: Some("permission request was already resolved".to_string()),
        });
    };
    let outcome = PermissionResolutionOutcome::Cancelled {
        decided_by: "relay".to_string(),
        reason_code: PERMISSION_CANCELLED_CODE.to_string(),
        reason: Some("relay shutdown cancelled pending permission request".to_string()),
    };
    if let Some(waiter) = take_waiter(permission_request_id)? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_permission_resolved_event(context, &record, &outcome);
    super::observability::publish_permission_queue_event(
        context.runtime_directory.as_path(),
        super::observability::PermissionQueueEvent::Invalidated {
            permission_request_id: record.permission_request_id.clone(),
            target_session: record.target_session.clone(),
            reason_code: PERMISSION_CANCELLED_CODE.to_string(),
        },
    );
    Ok(outcome)
}

fn emit_permission_requested_event(
    context: &PermissionEventContext,
    request: &PendingPermissionRequest,
) {
    for ui_session_id in &context.authorized_ui_sessions {
        let event =
            permission_requested_event(ui_session_id.as_str(), &context.bundle_name, request);
        let _ = send_event_to_registered_ui(context.bundle_name.as_str(), ui_session_id, &event);
    }
    emit_inscription(
        "relay.permission.requested",
        &json!({
            "bundle_name": context.bundle_name,
            "permission_request_id": request.permission_request_id,
            "message_id": request.message_id,
            "target_session": request.target_session,
            "requested_kind": request.requested_kind,
            "enqueued_at": request.enqueued_at,
        }),
    );
}

fn emit_permission_resolved_event(
    context: &PermissionEventContext,
    request: &PendingPermissionRequest,
    outcome: &PermissionResolutionOutcome,
) {
    let (outcome_label, reason_code, decided_by, reason) = match outcome {
        PermissionResolutionOutcome::Selected { decided_by, .. } => (
            "selected",
            Value::Null,
            Value::String(decided_by.clone()),
            Value::Null,
        ),
        PermissionResolutionOutcome::Cancelled {
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
            event_type: "permission.resolved".to_string(),
            bundle_name: context.bundle_name.clone(),
            target_session: ui_session_id.clone(),
            created_at: timestamp_rfc3339(),
            payload: json!({
                "message_id": request.message_id,
                "permission_request_id": request.permission_request_id,
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
        "relay.permission.resolved",
        &json!({
            "bundle_name": context.bundle_name,
            "permission_request_id": request.permission_request_id,
            "message_id": request.message_id,
            "outcome": outcome_label,
        }),
    );
}

fn permission_requested_event(
    ui_session_id: &str,
    bundle_name: &str,
    request: &PendingPermissionRequest,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: "permission.requested".to_string(),
        bundle_name: bundle_name.to_string(),
        target_session: ui_session_id.to_string(),
        created_at: timestamp_rfc3339(),
        payload: json!({
            "message_id": request.message_id,
            "permission_request_id": request.permission_request_id,
            "target_session": canonical_session_id(request.target_session.as_str(), bundle_name),
            "requested_kind": request.requested_kind,
            "requested_details": request.requested_details,
            "enqueued_at": request.enqueued_at,
        }),
    }
}

fn register_waiter(permission_request_id: &str) -> Result<(), String> {
    let mut waiters = permission_waiters()
        .lock()
        .map_err(|_| "failed to lock permission waiters".to_string())?;
    waiters.insert(
        permission_request_id.to_string(),
        Arc::new((Mutex::new(None), Condvar::new())),
    );
    Ok(())
}

fn get_waiter(permission_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let waiters = permission_waiters()
        .lock()
        .map_err(|_| "failed to lock permission waiters".to_string())?;
    Ok(waiters.get(permission_request_id).cloned())
}

fn take_waiter(permission_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let mut waiters = permission_waiters()
        .lock()
        .map_err(|_| "failed to lock permission waiters".to_string())?;
    Ok(waiters.remove(permission_request_id))
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
