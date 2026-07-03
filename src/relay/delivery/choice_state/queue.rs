//! Per-bundle choices queue state.
//!
//! Process-local map of in-flight `PendingChoiceRequest`s keyed by
//! `runtime_directory`. The queue is the relay's authoritative view of the
//! ACP server's outstanding choice requests; the ACP side is consulted via
//! `build_acp_chooser` only when the operator acts.
//!
//! Queue helpers, in declaration order:
//! - `choices_queues` accessor for the underlying `OnceLock<Mutex<...>>`.
//! - `with_queue_state` reads, edits, and re-stores one bundle's queue state
//!   atomically under the global lock.
//! - `sort_pending_by_sequence` orders the pending list so the snapshot
//!   emission and choice-resolution paths see a stable ordering.
//! - `pending_choice_option_ids` extracts option ids from a request's details,
//!   for the choice-pick authorization check.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde_json::Value;

use super::{ChoicesQueueState, PendingChoiceRequest};

static CHOICES_QUEUES: OnceLock<Mutex<HashMap<PathBuf, ChoicesQueueState>>> = OnceLock::new();

fn choices_queues() -> &'static Mutex<HashMap<PathBuf, ChoicesQueueState>> {
    CHOICES_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

// Holds the global queues lock for the duration of `mutate`, so callers can
// read, edit, and re-store one bundle's queue state atomically. Replaces the
// prior file lock + load + store sequence.
pub(super) fn with_queue_state<R>(
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

pub(super) fn sort_pending_by_sequence(pending: &mut [PendingChoiceRequest]) {
    pending.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then(left.choice_request_id.cmp(&right.choice_request_id))
    });
}

pub(super) fn pending_choice_option_ids(record: &PendingChoiceRequest) -> Vec<String> {
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
