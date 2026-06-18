use super::ReplayEntry;
use crate::transports::StructuredEntry;

pub fn replay_entries_to_snapshot_entries(entries: &[ReplayEntry]) -> Vec<StructuredEntry> {
    entries
        .iter()
        .map(|entry| match entry {
            ReplayEntry::User { lines } => StructuredEntry::User {
                lines: lines.clone(),
            },
            ReplayEntry::Agent { lines } => StructuredEntry::Agent {
                lines: lines.clone(),
            },
            ReplayEntry::Cognition { lines } => StructuredEntry::Cognition {
                lines: lines.clone(),
            },
            ReplayEntry::Invocation {
                call_id,
                status,
                invocation,
                result,
            } => StructuredEntry::Invocation {
                call_id: call_id.clone(),
                status: status.clone(),
                invocation: invocation.clone(),
                result: result.clone(),
            },
            ReplayEntry::Update { update_kind, lines } => StructuredEntry::Update {
                update_kind: update_kind.clone(),
                lines: lines.clone(),
            },
        })
        .collect()
}

pub fn snapshot_entries_to_plain_lines(entries: &[StructuredEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in entries {
        match entry {
            StructuredEntry::User { lines: value }
            | StructuredEntry::Agent { lines: value }
            | StructuredEntry::Cognition { lines: value }
            | StructuredEntry::Update { lines: value, .. } => {
                lines.extend(value.clone());
            }
            StructuredEntry::Invocation {
                call_id,
                status,
                invocation,
                result,
            } => {
                lines.push(format!(
                    "invocation {} {:?} {}",
                    call_id,
                    status,
                    serde_json::to_string(invocation).unwrap_or_else(|_| "{}".to_string())
                ));
                if let Some(result) = result {
                    lines.push(format!(
                        "result {}",
                        serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string())
                    ));
                }
            }
        }
    }
    lines
}
