use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};

use super::StartupFailureRecord;

const STARTUP_FAILURE_HISTORY_FILE: &str = "startup_failures.json";
const STARTUP_FAILURE_HISTORY_SCHEMA_VERSION: u32 = 1;
const MAX_STARTUP_FAILURES: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedStartupFailureHistory {
    schema_version: u32,
    next_sequence: u64,
    records: Vec<StartupFailureRecord>,
}

static STARTUP_FAILURE_HISTORY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn startup_failure_history_lock() -> &'static Mutex<()> {
    STARTUP_FAILURE_HISTORY_LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn load_startup_failures(
    runtime_directory: &Path,
) -> Result<Vec<StartupFailureRecord>, String> {
    let _guard = startup_failure_history_lock()
        .lock()
        .map_err(|_| "failed to lock startup failure history".to_string())?;
    let path = startup_failure_history_path(runtime_directory);
    let history = load_persisted_startup_failure_history(path.as_path())?;
    Ok(history.map_or_else(Vec::new, |value| value.records))
}

pub(super) fn append_startup_failure(
    runtime_directory: &Path,
    mut record: StartupFailureRecord,
) -> Result<StartupFailureRecord, String> {
    let _guard = startup_failure_history_lock()
        .lock()
        .map_err(|_| "failed to lock startup failure history".to_string())?;
    let path = startup_failure_history_path(runtime_directory);
    let mut history = load_persisted_startup_failure_history(path.as_path())?.unwrap_or(
        PersistedStartupFailureHistory {
            schema_version: STARTUP_FAILURE_HISTORY_SCHEMA_VERSION,
            next_sequence: 1,
            records: Vec::new(),
        },
    );

    history.schema_version = STARTUP_FAILURE_HISTORY_SCHEMA_VERSION;
    record.sequence = history.next_sequence;
    history.next_sequence = history.next_sequence.saturating_add(1);
    history.records.push(record.clone());
    if history.records.len() > MAX_STARTUP_FAILURES {
        let overflow = history.records.len() - MAX_STARTUP_FAILURES;
        history.records.drain(0..overflow);
    }

    store_persisted_startup_failure_history(path.as_path(), &history)?;
    Ok(record)
}

fn startup_failure_history_path(runtime_directory: &Path) -> PathBuf {
    runtime_directory.join(STARTUP_FAILURE_HISTORY_FILE)
}

fn load_persisted_startup_failure_history(
    path: &Path,
) -> Result<Option<PersistedStartupFailureHistory>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|source| {
        format!(
            "read startup failure history {} failed: {source}",
            path.display()
        )
    })?;
    let history =
        serde_json::from_str::<PersistedStartupFailureHistory>(raw.as_str()).map_err(|source| {
            format!(
                "parse startup failure history {} failed: {source}",
                path.display()
            )
        })?;
    if history.schema_version != STARTUP_FAILURE_HISTORY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported startup failure history schema_version '{}' in {}",
            history.schema_version,
            path.display()
        ));
    }
    Ok(Some(history))
}

fn store_persisted_startup_failure_history(
    path: &Path,
    history: &PersistedStartupFailureHistory,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            format!(
                "create startup failure history directory {} failed: {source}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_string_pretty(history).map_err(|source| {
        format!(
            "encode startup failure history {} failed: {source}",
            path.display()
        )
    })?;
    fs::write(path, encoded).map_err(|source| {
        format!(
            "write startup failure history {} failed: {source}",
            path.display()
        )
    })
}
