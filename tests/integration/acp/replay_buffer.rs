use std::collections::HashMap;

use agentmux::acp::replay::append_replay_entries;
use agentmux::acp::{PendingToolCall, REPLAY_BUFFER_MAX_ENTRIES, ReplayEntry, UserSource};

fn user_entry(label: &str) -> ReplayEntry {
    user_entry_with_source(label, UserSource::ReaderThread)
}

fn user_entry_with_source(label: &str, source: UserSource) -> ReplayEntry {
    ReplayEntry::User {
        lines: vec![label.to_string()],
        source,
    }
}

fn buffer_labels(buffer: &[ReplayEntry]) -> Vec<String> {
    buffer
        .iter()
        .map(|entry| match entry {
            ReplayEntry::User { lines, .. } => lines.join(""),
            _ => panic!("unexpected entry kind in test buffer"),
        })
        .collect()
}

#[test]
fn append_below_cap_preserves_order() {
    let mut buffer: Vec<ReplayEntry> = Vec::new();
    let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
    let incoming = (0..5).map(|i| user_entry(&format!("e{i}"))).collect();
    append_replay_entries(&mut buffer, &mut pending_calls, incoming);
    assert_eq!(
        buffer_labels(&buffer),
        vec!["e0", "e1", "e2", "e3", "e4"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
}

#[test]
fn append_at_cap_evicts_oldest_on_overflow() {
    let mut buffer: Vec<ReplayEntry> = (0..REPLAY_BUFFER_MAX_ENTRIES)
        .map(|i| user_entry(&format!("e{i}")))
        .collect();
    let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
    append_replay_entries(
        &mut buffer,
        &mut pending_calls,
        vec![user_entry("e_overflow")],
    );
    assert_eq!(buffer.len(), REPLAY_BUFFER_MAX_ENTRIES);
    let labels = buffer_labels(&buffer);
    assert_eq!(labels.first().map(String::as_str), Some("e1"));
    assert_eq!(labels.last().map(String::as_str), Some("e_overflow"));
}

#[test]
fn append_batch_exceeding_cap_evicts_proportionally() {
    let mut buffer: Vec<ReplayEntry> = (0..REPLAY_BUFFER_MAX_ENTRIES)
        .map(|i| user_entry(&format!("e{i}")))
        .collect();
    let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
    let incoming = (0..5).map(|i| user_entry(&format!("new{i}"))).collect();
    append_replay_entries(&mut buffer, &mut pending_calls, incoming);
    assert_eq!(buffer.len(), REPLAY_BUFFER_MAX_ENTRIES);
    let labels = buffer_labels(&buffer);
    assert_eq!(labels.first().map(String::as_str), Some("e5"));
    assert_eq!(labels.last().map(String::as_str), Some("new4"));
}
