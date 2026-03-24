use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const ACP_LOOK_SNAPSHOT_MAX_LINES: usize = 1000;
const ACP_SESSION_STATE_SCHEMA_VERSION: u32 = 1;
const ACP_SESSIONS_DIRECTORY: &str = "sessions";
const ACP_SESSION_STATE_FILE: &str = "state.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PersistedAcpSessionState {
    pub schema_version: u32,
    pub acp_session_id: String,
    #[serde(default = "default_acp_worker_readiness_state")]
    pub worker_state: AcpWorkerReadinessState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshot_lines: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AcpWorkerReadinessState {
    Available,
    Busy,
    Unavailable,
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

pub(in crate::relay) fn load_acp_snapshot_lines_for_look(
    runtime_socket_path: &Path,
    target_session: &str,
    requested_lines: usize,
) -> Result<Vec<String>, String> {
    let path = resolve_acp_session_state_path(runtime_socket_path, target_session)?;
    let _guard = acp_session_state_lock()
        .lock()
        .map_err(|_| "failed to lock ACP session state".to_string())?;
    let state = load_persisted_acp_session_state(path.as_path())?;
    let Some(state) = state else {
        return Ok(Vec::new());
    };
    let count = state.snapshot_lines.len();
    if requested_lines >= count {
        return Ok(state.snapshot_lines);
    }
    Ok(state.snapshot_lines[count - requested_lines..].to_vec())
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
        });
    state.schema_version = ACP_SESSION_STATE_SCHEMA_VERSION;
    state.acp_session_id = session_id.to_string();
    append_snapshot_lines(
        &mut state.snapshot_lines,
        snapshot_lines,
        ACP_LOOK_SNAPSHOT_MAX_LINES,
    );
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
