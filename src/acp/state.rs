use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    acp::{ReplayEntry, replay_entries_to_snapshot_entries},
    runtime::inscriptions::emit_inscription,
    transports::{AcpWorkerReadinessState, LookFreshness, LookSnapshotSource},
};

const ACP_SESSION_STATE_SCHEMA_VERSION: u32 = 2;
const ACP_SESSIONS_DIRECTORY: &str = "sessions";
const ACP_SESSION_STATE_FILE: &str = "state.json";
pub(crate) const ACP_LOOK_PRIME_TIMEOUT_MS: u64 = 750;
pub(crate) const ACP_STARTUP_PRIME_TIMEOUT_MS: u64 = 10_000;
const ACP_STALE_REASON_WORKER_INITIALIZING: &str = "acp_worker_initializing";
const ACP_STALE_REASON_WORKER_UNAVAILABLE: &str = "acp_worker_unavailable";
const ACP_STALE_REASON_WORKER_RECOVERING: &str = "acp_worker_recovering";
const ACP_STALE_REASON_SNAPSHOT_PRIME_TIMEOUT: &str = "acp_snapshot_prime_timeout";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedAcpSessionState {
    pub schema_version: u32,
    pub acp_session_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AcpLookSnapshot {
    pub snapshot_entries: Vec<crate::transports::StructuredEntry>,
    pub entries_total: usize,
    pub returned_entries_count: usize,
    pub freshness: LookFreshness,
    pub snapshot_source: LookSnapshotSource,
    pub stale_reason_code: Option<String>,
    pub snapshot_age_ms: Option<u64>,
}

use std::sync::{Mutex, OnceLock};

static ACP_SESSION_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn acp_session_state_lock() -> &'static Mutex<()> {
    ACP_SESSION_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

fn resolve_acp_session_state_path(
    runtime_directory: &Path,
    target_session: &str,
) -> Result<PathBuf, String> {
    Ok(runtime_directory
        .join(ACP_SESSIONS_DIRECTORY)
        .join(target_session)
        .join(ACP_SESSION_STATE_FILE))
}

pub(crate) fn load_persisted_acp_session_id(
    runtime_directory: &Path,
    target_session: &str,
) -> Result<Option<String>, String> {
    let path = resolve_acp_session_state_path(runtime_directory, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    Ok(state.map(|value| value.acp_session_id))
}

pub(crate) fn persist_acp_session_id(
    runtime_directory: &Path,
    target_session: &str,
    session_id: &str,
) -> Result<(), String> {
    let path = resolve_acp_session_state_path(runtime_directory, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = PersistedAcpSessionState {
        schema_version: ACP_SESSION_STATE_SCHEMA_VERSION,
        acp_session_id: session_id.to_string(),
    };
    store_persisted_acp_session_state(path.as_path(), &state)
}

pub(crate) fn derive_acp_look_snapshot(
    worker_state: Option<AcpWorkerReadinessState>,
    snapshot: Option<&[ReplayEntry]>,
    requested_entries: usize,
    offset: usize,
    prime_timed_out: bool,
) -> AcpLookSnapshot {
    let entries = snapshot
        .map(replay_entries_to_snapshot_entries)
        .unwrap_or_default();
    let entries_total = entries.len();
    // Tail-N window: skip `offset` entries from the newest end, then take the
    // last `requested_entries` of what remains. `offset` past the buffer start
    // yields an empty window without underflow.
    let window_end = entries_total.saturating_sub(offset);
    let window_start = window_end.saturating_sub(requested_entries);
    let snapshot_entries = entries[window_start..window_end].to_vec();
    let returned_entries_count = snapshot_entries.len();
    // Source reflects whether the replay buffer holds any entries, independent
    // of whether this particular window landed on them; an over-large offset
    // still walked a live buffer.
    let has_buffer = entries_total > 0;
    let snapshot_source = if has_buffer {
        LookSnapshotSource::LiveBuffer
    } else {
        LookSnapshotSource::None
    };

    if matches!(
        worker_state,
        Some(AcpWorkerReadinessState::Unavailable) | None
    ) {
        return AcpLookSnapshot {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness: LookFreshness::Stale,
            snapshot_source,
            stale_reason_code: Some(ACP_STALE_REASON_WORKER_UNAVAILABLE.to_string()),
            snapshot_age_ms: None,
        };
    }

    if matches!(worker_state, Some(AcpWorkerReadinessState::Recovering)) {
        return AcpLookSnapshot {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness: LookFreshness::Stale,
            snapshot_source,
            stale_reason_code: Some(ACP_STALE_REASON_WORKER_RECOVERING.to_string()),
            snapshot_age_ms: None,
        };
    }

    if !has_buffer {
        let stale_reason = if prime_timed_out {
            ACP_STALE_REASON_SNAPSHOT_PRIME_TIMEOUT
        } else {
            ACP_STALE_REASON_WORKER_INITIALIZING
        };
        return AcpLookSnapshot {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness: LookFreshness::Stale,
            snapshot_source,
            stale_reason_code: Some(stale_reason.to_string()),
            snapshot_age_ms: None,
        };
    }

    let stale_reason_code = match worker_state {
        Some(AcpWorkerReadinessState::Busy) | Some(AcpWorkerReadinessState::Available) => None,
        Some(AcpWorkerReadinessState::Initializing) => {
            Some(ACP_STALE_REASON_WORKER_INITIALIZING.to_string())
        }
        Some(AcpWorkerReadinessState::Recovering) => {
            Some(ACP_STALE_REASON_WORKER_RECOVERING.to_string())
        }
        Some(AcpWorkerReadinessState::Unavailable) | None => {
            Some(ACP_STALE_REASON_WORKER_UNAVAILABLE.to_string())
        }
    };
    let freshness = if stale_reason_code.is_some() {
        LookFreshness::Stale
    } else {
        LookFreshness::Fresh
    };
    AcpLookSnapshot {
        snapshot_entries,
        entries_total,
        returned_entries_count,
        freshness,
        snapshot_source,
        stale_reason_code,
        snapshot_age_ms: None,
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
    let state = match serde_json::from_str::<PersistedAcpSessionState>(raw.as_str()) {
        Ok(state) => state,
        Err(source) => {
            emit_inscription(
                "relay.acp.session_state.parse_failed",
                &serde_json::json!({
                    "path": path.display().to_string(),
                    "cause": source.to_string(),
                }),
            );
            if let Err(remove_error) = fs::remove_file(path) {
                emit_inscription(
                    "relay.acp.session_state.remove_failed",
                    &serde_json::json!({
                        "path": path.display().to_string(),
                        "cause": remove_error.to_string(),
                    }),
                );
            }
            return Ok(None);
        }
    };
    if state.schema_version != ACP_SESSION_STATE_SCHEMA_VERSION {
        emit_inscription(
            "relay.acp.session_state.schema_mismatch",
            &serde_json::json!({
                "path": path.display().to_string(),
                "observed_schema_version": state.schema_version,
                "expected_schema_version": ACP_SESSION_STATE_SCHEMA_VERSION,
            }),
        );
        if let Err(remove_error) = fs::remove_file(path) {
            emit_inscription(
                "relay.acp.session_state.remove_failed",
                &serde_json::json!({
                    "path": path.display().to_string(),
                    "cause": remove_error.to_string(),
                }),
            );
        }
        return Ok(None);
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
