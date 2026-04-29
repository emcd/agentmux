use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    acp::PermissionOption,
    runtime::{inscriptions::emit_inscription, signals::shutdown_requested},
};

use super::super::{
    relay_error,
    stream::{RelayStreamEvent, send_event_to_registered_ui},
};

const PERMISSION_QUEUE_FILE: &str = "permission_queue.json";
const PERMISSION_QUEUE_SCHEMA_VERSION: u32 = 1;
const PERMISSION_CANCELLED_CODE: &str = "runtime_permission_request_cancelled";
const PERMISSION_DENIED_CODE: &str = "runtime_permission_request_denied";
const PERMISSION_ALREADY_RESOLVED_CODE: &str = "runtime_permission_request_already_resolved";
const PERMISSION_QUEUE_UNAVAILABLE_CODE: &str = "runtime_permission_queue_unavailable";
const PERMISSION_QUEUE_FULL_CODE: &str = "runtime_permission_queue_full";
const PERMISSION_WAIT_POLL_MS: u64 = 100;

type SharedWaiterState = Arc<(Mutex<Option<PermissionResolutionOutcome>>, Condvar)>;

// Queue state is persisted so permission requests survive relay restarts; the
// in-memory waiter map is only for active process-local request/response wakeups.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedPermissionQueueState {
    schema_version: u32,
    next_sequence: u64,
    pending: Vec<PersistedPendingPermissionRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::relay) struct PersistedPendingPermissionRequest {
    pub(in crate::relay) permission_request_id: String,
    pub(in crate::relay) message_id: String,
    pub(in crate::relay) target_session: String,
    pub(in crate::relay) requested_kind: String,
    pub(in crate::relay) requested_details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::relay) approve_option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::relay) deny_option_id: Option<String>,
    pub(in crate::relay) enqueued_at: String,
    pub(in crate::relay) enqueued_at_ms: i64,
    pub(in crate::relay) sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::relay) enum PermissionResolutionOutcome {
    Approved {
        option_id: String,
        decided_by: String,
    },
    Denied {
        option_id: String,
        decided_by: String,
        reason: Option<String>,
    },
    Cancelled {
        reason_code: String,
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PermissionEnqueueResult {
    pub(in crate::relay) permission_request_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum PermissionDecisionKind {
    Approve,
    Deny,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PermissionDecisionRequest {
    pub(in crate::relay) permission_request_id: String,
    pub(in crate::relay) option_id: Option<String>,
    pub(in crate::relay) decision: PermissionDecisionKind,
    pub(in crate::relay) decided_by: String,
    pub(in crate::relay) reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(in crate::relay) struct PermissionEventContext {
    pub(in crate::relay) runtime_directory: PathBuf,
    pub(in crate::relay) bundle_name: String,
    pub(in crate::relay) authorized_ui_sessions: Vec<String>,
}

static PERMISSION_QUEUE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PERMISSION_WAITERS: OnceLock<Mutex<HashMap<String, SharedWaiterState>>> = OnceLock::new();

fn sort_pending_by_sequence(pending: &mut [PersistedPendingPermissionRequest]) {
    pending.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then(left.permission_request_id.cmp(&right.permission_request_id))
    });
}

fn permission_queue_lock() -> &'static Mutex<()> {
    PERMISSION_QUEUE_LOCK.get_or_init(|| Mutex::new(()))
}

fn permission_waiters() -> &'static Mutex<HashMap<String, SharedWaiterState>> {
    PERMISSION_WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::relay) fn enqueue_permission_request(
    context: &PermissionEventContext,
    message_id: &str,
    target_session: &str,
    requested_kind: &str,
    requested_details: Value,
    options: &[PermissionOption],
    max_pending: usize,
) -> Result<PermissionEnqueueResult, String> {
    let _guard = permission_queue_lock()
        .lock()
        .map_err(|_| "failed to lock permission queue state".to_string())?;
    let path = permission_queue_path(context.runtime_directory.as_path());
    let mut state = load_persisted_permission_queue_state(path.as_path())?.unwrap_or(
        PersistedPermissionQueueState {
            schema_version: PERMISSION_QUEUE_SCHEMA_VERSION,
            next_sequence: 1,
            pending: Vec::new(),
        },
    );
    if state.pending.len() >= max_pending {
        return Err(PERMISSION_QUEUE_FULL_CODE.to_string());
    }
    let permission_request_id = Uuid::new_v4().to_string();
    let now = current_timestamp_millis();
    // ACP Permission Options kinds:
    // - allow_once / allow_always
    // - reject_once / reject_always
    // We persist preferred option ids so UI decisions can map back to ACP
    // "selected" outcomes with a deterministic optionId.
    // Ref: https://agentclientprotocol.com/protocol/tool-calls.md
    let approve_option_id = options
        .iter()
        .find(|option| matches!(option.kind.as_str(), "allow_once" | "allow_always"))
        .or_else(|| options.first())
        .map(|option| option.option_id.clone());
    let deny_option_id = options
        .iter()
        .find(|option| matches!(option.kind.as_str(), "reject_once" | "reject_always"))
        .or_else(|| {
            options
                .iter()
                .find(|option| !matches!(option.kind.as_str(), "allow_once" | "allow_always"))
        })
        .map(|option| option.option_id.clone());
    let record = PersistedPendingPermissionRequest {
        permission_request_id: permission_request_id.clone(),
        message_id: message_id.to_string(),
        target_session: target_session.to_string(),
        requested_kind: requested_kind.to_string(),
        requested_details,
        approve_option_id,
        deny_option_id,
        enqueued_at: timestamp_rfc3339(),
        enqueued_at_ms: now,
        sequence: state.next_sequence,
    };
    state.next_sequence = state.next_sequence.saturating_add(1);
    state.pending.push(record.clone());
    sort_pending_by_sequence(state.pending.as_mut_slice());
    store_persisted_permission_queue_state(path.as_path(), &state)?;
    register_waiter(permission_request_id.as_str())?;
    emit_permission_requested_event(context, &record);
    Ok(PermissionEnqueueResult {
        permission_request_id,
    })
}

pub(in crate::relay) fn resolve_permission_request(
    context: &PermissionEventContext,
    decision: PermissionDecisionRequest,
) -> Result<PermissionResolutionOutcome, String> {
    let _guard = permission_queue_lock()
        .lock()
        .map_err(|_| "failed to lock permission queue state".to_string())?;
    let path = permission_queue_path(context.runtime_directory.as_path());
    let mut state = load_persisted_permission_queue_state(path.as_path())?.ok_or_else(|| {
        format!("{PERMISSION_QUEUE_UNAVAILABLE_CODE}: permission queue state is unavailable")
    })?;

    let index = state
        .pending
        .iter()
        .position(|record| record.permission_request_id == decision.permission_request_id)
        .ok_or_else(|| PERMISSION_ALREADY_RESOLVED_CODE.to_string())?;
    let record = state.pending.remove(index);
    store_persisted_permission_queue_state(path.as_path(), &state)?;

    let outcome = match decision.decision {
        PermissionDecisionKind::Approve => {
            if let Some(option_id) = decision.option_id.or(record.approve_option_id.clone()) {
                PermissionResolutionOutcome::Approved {
                    option_id,
                    decided_by: decision.decided_by.clone(),
                }
            } else {
                PermissionResolutionOutcome::Cancelled {
                    reason_code: PERMISSION_CANCELLED_CODE.to_string(),
                    reason: "no ACP approval option is available".to_string(),
                }
            }
        }
        PermissionDecisionKind::Deny => {
            if let Some(option_id) = decision.option_id.or(record.deny_option_id.clone()) {
                PermissionResolutionOutcome::Denied {
                    option_id,
                    decided_by: decision.decided_by.clone(),
                    reason: decision.reason.clone(),
                }
            } else {
                PermissionResolutionOutcome::Cancelled {
                    reason_code: PERMISSION_CANCELLED_CODE.to_string(),
                    reason: "no ACP denial option is available".to_string(),
                }
            }
        }
    };

    if let Some(waiter) = take_waiter(decision.permission_request_id.as_str())? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_permission_resolved_event(context, &record, &outcome);
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

pub(in crate::relay) fn list_pending_permission_requests(
    runtime_directory: &Path,
) -> Result<Vec<PersistedPendingPermissionRequest>, String> {
    let _guard = permission_queue_lock()
        .lock()
        .map_err(|_| "failed to lock permission queue state".to_string())?;
    let path = permission_queue_path(runtime_directory);
    let state = load_persisted_permission_queue_state(path.as_path())?;
    Ok(state.map_or_else(Vec::new, |value| value.pending))
}

fn cancel_permission_request_on_shutdown(
    context: &PermissionEventContext,
    permission_request_id: &str,
) -> Result<PermissionResolutionOutcome, String> {
    let _guard = permission_queue_lock()
        .lock()
        .map_err(|_| "failed to lock permission queue state".to_string())?;
    let path = permission_queue_path(context.runtime_directory.as_path());
    let Some(mut state) = load_persisted_permission_queue_state(path.as_path())? else {
        return Ok(PermissionResolutionOutcome::Cancelled {
            reason_code: PERMISSION_CANCELLED_CODE.to_string(),
            reason: "relay shutdown cancelled pending permission request".to_string(),
        });
    };
    let Some(index) = state
        .pending
        .iter()
        .position(|record| record.permission_request_id == permission_request_id)
    else {
        return Ok(PermissionResolutionOutcome::Cancelled {
            reason_code: PERMISSION_ALREADY_RESOLVED_CODE.to_string(),
            reason: "permission request was already resolved".to_string(),
        });
    };
    let record = state.pending.remove(index);
    store_persisted_permission_queue_state(path.as_path(), &state)?;
    let outcome = PermissionResolutionOutcome::Cancelled {
        reason_code: PERMISSION_CANCELLED_CODE.to_string(),
        reason: "relay shutdown cancelled pending permission request".to_string(),
    };
    if let Some(waiter) = take_waiter(permission_request_id)? {
        let (lock, condvar) = &*waiter;
        if let Ok(mut value) = lock.lock() {
            *value = Some(outcome.clone());
            condvar.notify_all();
        }
    }
    emit_permission_resolved_event(context, &record, &outcome);
    Ok(outcome)
}

fn emit_permission_requested_event(
    context: &PermissionEventContext,
    request: &PersistedPendingPermissionRequest,
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
    request: &PersistedPendingPermissionRequest,
    outcome: &PermissionResolutionOutcome,
) {
    let (outcome_label, reason_code, decided_by, reason) = match outcome {
        PermissionResolutionOutcome::Approved { decided_by, .. } => (
            "approved",
            Value::Null,
            Value::String(decided_by.clone()),
            Value::Null,
        ),
        PermissionResolutionOutcome::Denied {
            decided_by, reason, ..
        } => (
            "denied",
            Value::String(PERMISSION_DENIED_CODE.to_string()),
            Value::String(decided_by.clone()),
            reason.clone().map(Value::String).unwrap_or(Value::Null),
        ),
        PermissionResolutionOutcome::Cancelled {
            reason_code,
            reason,
        } => (
            "cancelled",
            Value::String(reason_code.clone()),
            Value::String("relay".to_string()),
            Value::String(reason.clone()),
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
    request: &PersistedPendingPermissionRequest,
) -> RelayStreamEvent {
    RelayStreamEvent {
        event_type: "permission.requested".to_string(),
        bundle_name: bundle_name.to_string(),
        target_session: ui_session_id.to_string(),
        created_at: timestamp_rfc3339(),
        payload: json!({
            "message_id": request.message_id,
            "permission_request_id": request.permission_request_id,
            "target_session": request.target_session,
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

fn permission_queue_path(runtime_directory: &Path) -> PathBuf {
    runtime_directory.join(PERMISSION_QUEUE_FILE)
}

fn load_persisted_permission_queue_state(
    path: &Path,
) -> Result<Option<PersistedPermissionQueueState>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|source| {
        format!(
            "read permission queue state {} failed: {source}",
            path.display()
        )
    })?;
    let mut state =
        serde_json::from_str::<PersistedPermissionQueueState>(raw.as_str()).map_err(|source| {
            format!(
                "parse permission queue state {} failed: {source}",
                path.display()
            )
        })?;
    if state.schema_version != PERMISSION_QUEUE_SCHEMA_VERSION {
        return Err(format!(
            "{}: unsupported permission queue schema_version '{}' in {}",
            PERMISSION_QUEUE_UNAVAILABLE_CODE,
            state.schema_version,
            path.display()
        ));
    }
    sort_pending_by_sequence(state.pending.as_mut_slice());
    Ok(Some(state))
}

fn store_persisted_permission_queue_state(
    path: &Path,
    state: &PersistedPermissionQueueState,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            format!(
                "create permission queue state directory {} failed: {source}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_string_pretty(state).map_err(|source| {
        format!(
            "encode permission queue state {} failed: {source}",
            path.display()
        )
    })?;
    fs::write(path, encoded).map_err(|source| {
        format!(
            "write permission queue state {} failed: {source}",
            path.display()
        )
    })
}

fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn current_timestamp_millis() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    i64::try_from(elapsed.as_millis()).unwrap_or(0)
}

pub(in crate::relay) fn map_permission_state_error(
    code: &str,
    message: &str,
) -> super::super::RelayError {
    match code {
        PERMISSION_QUEUE_FULL_CODE => relay_error(code, message, None),
        PERMISSION_ALREADY_RESOLVED_CODE => relay_error(code, message, None),
        PERMISSION_QUEUE_UNAVAILABLE_CODE => relay_error(code, message, None),
        _ => relay_error(
            "internal_unexpected_failure",
            message,
            Some(json!({ "code": code })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{PersistedPendingPermissionRequest, sort_pending_by_sequence};
    use serde_json::json;

    #[test]
    fn sort_pending_uses_sequence_as_fifo_source_of_truth() {
        let mut pending = vec![
            PersistedPendingPermissionRequest {
                permission_request_id: "perm-z".to_string(),
                message_id: "msg-2".to_string(),
                target_session: "alpha".to_string(),
                requested_kind: "other".to_string(),
                requested_details: json!({}),
                approve_option_id: Some("allow".to_string()),
                deny_option_id: Some("deny".to_string()),
                enqueued_at: "2026-01-01T00:00:00Z".to_string(),
                enqueued_at_ms: 12345,
                sequence: 2,
            },
            PersistedPendingPermissionRequest {
                permission_request_id: "perm-a".to_string(),
                message_id: "msg-1".to_string(),
                target_session: "alpha".to_string(),
                requested_kind: "other".to_string(),
                requested_details: json!({}),
                approve_option_id: Some("allow".to_string()),
                deny_option_id: Some("deny".to_string()),
                enqueued_at: "2026-01-01T00:00:00Z".to_string(),
                enqueued_at_ms: 12345,
                sequence: 1,
            },
        ];

        sort_pending_by_sequence(pending.as_mut_slice());

        assert_eq!(pending[0].sequence, 1);
        assert_eq!(pending[0].permission_request_id, "perm-a");
        assert_eq!(pending[1].sequence, 2);
        assert_eq!(pending[1].permission_request_id, "perm-z");
    }
}
