use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::relay::{AcpLookFreshness, AcpLookSnapshotSource};

const ACP_LOOK_SNAPSHOT_MAX_LINES: usize = 1000;
const ACP_SESSION_STATE_SCHEMA_VERSION: u32 = 1;
const ACP_SESSIONS_DIRECTORY: &str = "sessions";
const ACP_SESSION_STATE_FILE: &str = "state.json";
pub(in crate::relay) const ACP_LOOK_PRIME_TIMEOUT_MS: u64 = 750;
pub(in crate::relay) const ACP_STREAM_STALLED_AFTER_MS: u64 = 5000;
pub(in crate::relay) const ACP_STALE_REASON_WORKER_INITIALIZING: &str = "acp_worker_initializing";
pub(in crate::relay) const ACP_STALE_REASON_WORKER_UNAVAILABLE: &str = "acp_worker_unavailable";
pub(in crate::relay) const ACP_STALE_REASON_SNAPSHOT_PRIME_TIMEOUT: &str =
    "acp_snapshot_prime_timeout";
pub(in crate::relay) const ACP_STALE_REASON_STREAM_STALLED: &str = "acp_stream_stalled";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PersistedAcpSessionState {
    pub schema_version: u32,
    pub acp_session_id: String,
    #[serde(default = "default_acp_worker_readiness_state")]
    pub worker_state: AcpWorkerReadinessState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshot_lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snapshot_update_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AcpWorkerReadinessState {
    Initializing,
    Available,
    Busy,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::relay) struct AcpLookSnapshot {
    pub snapshot_lines: Vec<String>,
    pub freshness: AcpLookFreshness,
    pub snapshot_source: AcpLookSnapshotSource,
    pub stale_reason_code: Option<String>,
    pub snapshot_age_ms: Option<u64>,
}

fn default_acp_worker_readiness_state() -> AcpWorkerReadinessState {
    AcpWorkerReadinessState::Available
}

use std::sync::{Mutex, OnceLock};

static ACP_SESSION_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn acp_session_state_lock() -> &'static Mutex<()> {
    ACP_SESSION_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn resolve_acp_session_state_path(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<PathBuf, String> {
    let Some(runtime_directory) = runtime_socket_path.parent() else {
        return Err("runtime socket path has no parent runtime directory".to_string());
    };
    Ok(runtime_directory
        .join(ACP_SESSIONS_DIRECTORY)
        .join(target_session)
        .join(ACP_SESSION_STATE_FILE))
}

pub(in crate::relay) fn load_acp_worker_readiness_state(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<Option<AcpWorkerReadinessState>, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    Ok(state.map(|value| value.worker_state))
}

pub(super) fn load_persisted_acp_session_id(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<Option<String>, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    Ok(state.map(|value| value.acp_session_id))
}

pub(in crate::relay) fn acp_session_ready_for_startup(
    runtime_socket_path: &Path,
    target_session: &str,
) -> Result<bool, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let Some(state) = load_persisted_acp_session_state(path.as_path())? else {
        return Ok(false);
    };
    Ok(!state.acp_session_id.trim().is_empty()
        && matches!(state.worker_state, AcpWorkerReadinessState::Available))
}

pub(in crate::relay) fn load_acp_snapshot_lines_for_look(
    runtime_socket_path: &Path,
    target_session: &str,
    requested_lines: usize,
    prime_timed_out: bool,
) -> Result<AcpLookSnapshot, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    let Some(state) = state else {
        let stale_reason = if prime_timed_out {
            ACP_STALE_REASON_SNAPSHOT_PRIME_TIMEOUT
        } else {
            ACP_STALE_REASON_WORKER_UNAVAILABLE
        };
        return Ok(AcpLookSnapshot {
            snapshot_lines: Vec::new(),
            freshness: AcpLookFreshness::Stale,
            snapshot_source: AcpLookSnapshotSource::None,
            stale_reason_code: Some(stale_reason.to_string()),
            snapshot_age_ms: None,
        });
    };
    let count = state.snapshot_lines.len();
    let snapshot_lines = if requested_lines >= count {
        state.snapshot_lines
    } else {
        state.snapshot_lines[count - requested_lines..].to_vec()
    };
    let has_snapshot = !snapshot_lines.is_empty();
    let snapshot_source = if has_snapshot {
        AcpLookSnapshotSource::LiveBuffer
    } else {
        AcpLookSnapshotSource::None
    };
    let snapshot_age_ms = if has_snapshot {
        snapshot_age_millis(state.last_snapshot_update_ms)
    } else {
        None
    };

    if !has_snapshot {
        let stale_reason = if prime_timed_out {
            ACP_STALE_REASON_SNAPSHOT_PRIME_TIMEOUT
        } else {
            match state.worker_state {
                AcpWorkerReadinessState::Initializing => ACP_STALE_REASON_WORKER_INITIALIZING,
                AcpWorkerReadinessState::Unavailable => ACP_STALE_REASON_WORKER_UNAVAILABLE,
                AcpWorkerReadinessState::Available | AcpWorkerReadinessState::Busy => {
                    ACP_STALE_REASON_WORKER_INITIALIZING
                }
            }
        };
        return Ok(AcpLookSnapshot {
            snapshot_lines,
            freshness: AcpLookFreshness::Stale,
            snapshot_source,
            stale_reason_code: Some(stale_reason.to_string()),
            snapshot_age_ms,
        });
    }

    let stale_reason_code = if matches!(state.worker_state, AcpWorkerReadinessState::Unavailable) {
        Some(ACP_STALE_REASON_WORKER_UNAVAILABLE.to_string())
    } else if !matches!(state.worker_state, AcpWorkerReadinessState::Busy)
        && snapshot_age_ms.is_some_and(|age| age >= ACP_STREAM_STALLED_AFTER_MS)
    {
        Some(ACP_STALE_REASON_STREAM_STALLED.to_string())
    } else {
        None
    };
    let freshness = if stale_reason_code.is_some() {
        AcpLookFreshness::Stale
    } else {
        AcpLookFreshness::Fresh
    };
    Ok(AcpLookSnapshot {
        snapshot_lines,
        freshness,
        snapshot_source,
        stale_reason_code,
        snapshot_age_ms,
    })
}

pub(super) fn persist_acp_session_id(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: &str,
) -> Result<(), String> {
    persist_acp_snapshot_lines(runtime_socket_path, target_session, session_id, &[])
}

pub(super) fn persist_acp_worker_state(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: Option<&str>,
    worker_state: AcpWorkerReadinessState,
) -> Result<(), String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let mut state = match load_persisted_acp_session_state(path.as_path())? {
        Some(value) => value,
        None => {
            let Some(session_id) = session_id else {
                return Ok(());
            };
            PersistedAcpSessionState {
                schema_version: ACP_SESSION_STATE_SCHEMA_VERSION,
                acp_session_id: session_id.to_string(),
                worker_state: AcpWorkerReadinessState::Available,
                snapshot_lines: Vec::new(),
                last_snapshot_update_ms: None,
            }
        }
    };
    if let Some(session_id) = session_id {
        state.acp_session_id = session_id.to_string();
    }
    state.schema_version = ACP_SESSION_STATE_SCHEMA_VERSION;
    state.worker_state = worker_state;
    store_persisted_acp_session_state(path.as_path(), &state)
}

pub(super) fn persist_acp_snapshot_lines(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: &str,
    snapshot_lines: &[String],
) -> Result<(), String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let mut state =
        load_persisted_acp_session_state(path.as_path())?.unwrap_or(PersistedAcpSessionState {
            schema_version: ACP_SESSION_STATE_SCHEMA_VERSION,
            acp_session_id: session_id.to_string(),
            worker_state: AcpWorkerReadinessState::Available,
            snapshot_lines: Vec::new(),
            last_snapshot_update_ms: None,
        });
    state.schema_version = ACP_SESSION_STATE_SCHEMA_VERSION;
    state.acp_session_id = session_id.to_string();
    append_snapshot_lines(
        &mut state.snapshot_lines,
        snapshot_lines,
        ACP_LOOK_SNAPSHOT_MAX_LINES,
    );
    if !snapshot_lines.is_empty() {
        state.last_snapshot_update_ms = current_timestamp_millis();
    }
    store_persisted_acp_session_state(path.as_path(), &state)
}

pub(super) fn replace_acp_snapshot_lines(
    runtime_socket_path: &Path,
    target_session: &str,
    session_id: &str,
    snapshot_lines: &[String],
) -> Result<(), String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let mut state =
        load_persisted_acp_session_state(path.as_path())?.unwrap_or(PersistedAcpSessionState {
            schema_version: ACP_SESSION_STATE_SCHEMA_VERSION,
            acp_session_id: session_id.to_string(),
            worker_state: AcpWorkerReadinessState::Available,
            snapshot_lines: Vec::new(),
            last_snapshot_update_ms: None,
        });
    state.schema_version = ACP_SESSION_STATE_SCHEMA_VERSION;
    state.acp_session_id = session_id.to_string();
    state.snapshot_lines = snapshot_lines.to_vec();
    if state.snapshot_lines.len() > ACP_LOOK_SNAPSHOT_MAX_LINES {
        let overflow = state.snapshot_lines.len() - ACP_LOOK_SNAPSHOT_MAX_LINES;
        state.snapshot_lines.drain(0..overflow);
    }
    if !snapshot_lines.is_empty() {
        state.last_snapshot_update_ms = current_timestamp_millis();
    }
    store_persisted_acp_session_state(path.as_path(), &state)
}

fn append_snapshot_lines(storage: &mut Vec<String>, appended: &[String], max_lines: usize) {
    storage.extend(appended.iter().cloned());
    if storage.len() > max_lines {
        let overflow = storage.len() - max_lines;
        storage.drain(0..overflow);
    }
}

fn load_persisted_acp_session_state(
    path: &Path,
) -> Result<Option<PersistedAcpSessionState>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|source| format!("read ACP session state {} failed: {source}", path.display()))?;
    let state =
        serde_json::from_str::<PersistedAcpSessionState>(raw.as_str()).map_err(|source| {
            format!(
                "parse ACP session state {} failed: {source}",
                path.display()
            )
        })?;
    if state.schema_version != ACP_SESSION_STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported ACP session state schema_version '{}' in {}",
            state.schema_version,
            path.display()
        ));
    }
    if state.acp_session_id.trim().is_empty() {
        return Err(format!(
            "invalid ACP session state {}: acp_session_id must be non-empty",
            path.display()
        ));
    }
    Ok(Some(state))
}

fn store_persisted_acp_session_state(
    path: &Path,
    state: &PersistedAcpSessionState,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            format!(
                "create ACP session state directory {} failed: {source}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_string_pretty(state).map_err(|source| {
        format!(
            "encode ACP session state {} failed: {source}",
            path.display()
        )
    })?;
    fs::write(path, encoded).map_err(|source| {
        format!(
            "write ACP session state {} failed: {source}",
            path.display()
        )
    })
}

fn current_timestamp_millis() -> Option<i64> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

fn snapshot_age_millis(updated_at_ms: Option<i64>) -> Option<u64> {
    let now_ms = current_timestamp_millis()?;
    let updated_at_ms = updated_at_ms?;
    if updated_at_ms > now_ms {
        return None;
    }
    u64::try_from(now_ms - updated_at_ms).ok()
}
