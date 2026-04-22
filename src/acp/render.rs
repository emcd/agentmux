use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ReplayEntry;

/// Canonical ACP snapshot entry used by relay look payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpSnapshotEntry {
    User {
        lines: Vec<String>,
    },
    Agent {
        lines: Vec<String>,
    },
    Cognition {
        lines: Vec<String>,
    },
    Invocation {
        invocation: Value,
    },
    Result {
        result: Value,
    },
    Update {
        update_kind: String,
        lines: Vec<String>,
    },
}

pub fn replay_entries_to_snapshot_entries(entries: &[ReplayEntry]) -> Vec<AcpSnapshotEntry> {
    entries
        .iter()
        .map(|entry| match entry {
            ReplayEntry::User { lines } => AcpSnapshotEntry::User {
                lines: lines.clone(),
            },
            ReplayEntry::Agent { lines } => AcpSnapshotEntry::Agent {
                lines: lines.clone(),
            },
            ReplayEntry::Cognition { lines } => AcpSnapshotEntry::Cognition {
                lines: lines.clone(),
            },
            ReplayEntry::Invocation { invocation } => AcpSnapshotEntry::Invocation {
                invocation: invocation.clone(),
            },
            ReplayEntry::Result { result } => AcpSnapshotEntry::Result {
                result: result.clone(),
            },
            ReplayEntry::Update { update_kind, lines } => AcpSnapshotEntry::Update {
                update_kind: update_kind.clone(),
                lines: lines.clone(),
            },
        })
        .collect()
}

pub fn snapshot_entries_to_plain_lines(entries: &[AcpSnapshotEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in entries {
        match entry {
            AcpSnapshotEntry::User { lines: value }
            | AcpSnapshotEntry::Agent { lines: value }
            | AcpSnapshotEntry::Cognition { lines: value }
            | AcpSnapshotEntry::Update { lines: value, .. } => {
                lines.extend(value.clone());
            }
            AcpSnapshotEntry::Invocation { invocation } => {
                lines.push(format!(
                    "invocation {}",
                    serde_json::to_string(invocation).unwrap_or_else(|_| "{}".to_string())
                ));
            }
            AcpSnapshotEntry::Result { result } => {
                lines.push(format!(
                    "result {}",
                    serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string())
                ));
            }
        }
    }
    lines
}
