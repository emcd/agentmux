use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};

use super::StartupFailureRecord;
use crate::runtime::inscriptions::emit_inscription;

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

// Per-process record of (runtime_directory, session_id) pairs whose startup
// failure entries have already been cleared and not since re-recorded.
// Lets repeated successful serves skip the file lock + read/write entirely.
// The set is bounded by the number of distinct sessions a relay handles.
//
// This is a cache of "the history holds nothing for this session", not a
// record of having once cleared it. `append_startup_failure` is what makes
// that statement false again, so it is what evicts the entry — otherwise a
// session that fails, recovers, and fails again keeps the second failure
// forever, because the recovery that would clear it is deduplicated away.
static SERVED_SESSIONS_CLEARED: OnceLock<Mutex<HashSet<(PathBuf, String)>>> = OnceLock::new();

fn startup_failure_history_lock() -> &'static Mutex<()> {
    STARTUP_FAILURE_HISTORY_LOCK.get_or_init(|| Mutex::new(()))
}

fn served_sessions_cleared() -> &'static Mutex<HashSet<(PathBuf, String)>> {
    SERVED_SESSIONS_CLEARED.get_or_init(|| Mutex::new(HashSet::new()))
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

/// Records that `session_id` has been observed serving successfully and clears
/// any stale startup failure entries from its bundle history.
///
/// Repeated calls collapse to a mutex acquisition while the session's history
/// is known-empty, so the hot delivery path does not pay a file lock and a
/// read/write per delivery. The cache is re-armed by a newly recorded failure
/// rather than lasting the process's lifetime: a session can fail more than
/// once, and each recovery has to be able to clear the failure that preceded it.
///
/// Lock order: this acquires the served-sessions cache and **releases it**
/// before [`clear_startup_failures_for_session`] takes the history lock.
/// [`append_startup_failure`] nests the other way, holding the history lock
/// while it evicts. Only that one nesting direction exists, so the two cannot
/// deadlock — keep it that way.
pub(super) fn note_session_served_successfully(
    runtime_directory: &Path,
    session_id: &str,
) -> Result<(), String> {
    let key = (runtime_directory.to_path_buf(), session_id.to_string());
    {
        let mut guard = served_sessions_cleared()
            .lock()
            .map_err(|_| "failed to lock served sessions cache".to_string())?;
        if !guard.insert(key) {
            return Ok(());
        }
    }
    clear_startup_failures_for_session(runtime_directory, session_id).map(|_| ())
}

pub(super) fn clear_startup_failures_for_session(
    runtime_directory: &Path,
    session_id: &str,
) -> Result<usize, String> {
    let _guard = startup_failure_history_lock()
        .lock()
        .map_err(|_| "failed to lock startup failure history".to_string())?;
    let path = startup_failure_history_path(runtime_directory);
    let Some(mut history) = load_persisted_startup_failure_history(path.as_path())? else {
        return Ok(0);
    };
    let original_len = history.records.len();
    history
        .records
        .retain(|record| record.session_id != session_id);
    let removed = original_len - history.records.len();
    if removed > 0 {
        store_persisted_startup_failure_history(path.as_path(), &history)?;
    }
    Ok(removed)
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
    // After the write, not before, and while the history lock is still held.
    // Evicting first would leave a window where a concurrent successful serve
    // clears the history, re-inserts the cache entry, and only then sees this
    // record land — stranding it exactly as before. Evicting after means the
    // worst a race can do is clear a failure a moment too eagerly, which the
    // next failure re-records, rather than keep one that no longer applies.
    forget_served_session(runtime_directory, record.session_id.as_str())?;
    Ok(record)
}

/// Drops the served-session cache entry for one session, so the next observed
/// successful serve reaches the history again instead of being deduplicated.
fn forget_served_session(runtime_directory: &Path, session_id: &str) -> Result<(), String> {
    let mut guard = served_sessions_cleared()
        .lock()
        .map_err(|_| "failed to lock served sessions cache".to_string())?;
    guard.remove(&(runtime_directory.to_path_buf(), session_id.to_string()));
    Ok(())
}

/// Persists each recorded per-session startup failure to `runtime_directory`'s
/// history and announces it, returning the persisted records with their assigned
/// timestamps and sequences.
///
/// Every path that brings a bundle up records its per-session failures through
/// here — relay-host autostart and reconcile/`up` alike — so a failure reported by
/// one surface is the same record `list` and the TUI later read, and the
/// `relay.session_start_failed` inscription has exactly one emitter.
pub(super) fn persist_and_announce_startup_failures(
    bundle_name: &str,
    runtime_directory: &Path,
    failures: &[StartupFailureRecord],
) -> Result<Vec<StartupFailureRecord>, String> {
    let mut persisted_records = Vec::with_capacity(failures.len());
    for failure in failures {
        let persisted = append_startup_failure(runtime_directory, failure.clone())?;
        emit_inscription(
            "relay.session_start_failed",
            &serde_json::json!({
                "bundle_name": bundle_name,
                "session_id": persisted.session_id,
                "transport": persisted.transport,
                "code": persisted.code,
                "reason": persisted.reason,
                "timestamp": persisted.timestamp,
                "sequence": persisted.sequence,
                "details": persisted.details,
            }),
        );
        persisted_records.push(persisted);
    }
    Ok(persisted_records)
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
