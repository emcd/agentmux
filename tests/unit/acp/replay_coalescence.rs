//! Unit coverage for `coalesce_replay_entries_on_append` (the
//! reader-thread helper) and the prompt-path non-coalescing
//! `append_replay_entries`.
//!
//! The helper walks `new_entries` and merges same-kind adjacency with the
//! buffer tail, preserving all line content in receive order. Rules:
//!
//! - `User`, `Agent`, `Cognition`: adjacent same-kind entries merge by
//!   `lines` extension. `User` merges only when both entries share the
//!   same `UserSource` (cross-source adjacency never merges).
//! - `Update`: adjacent entries merge only when `update_kind` matches.
//! - `Invocation`: never merges (per-call boundary must be preserved).
//! - Different-kind adjacency: never merges.
//! - The 1000-entry cap is enforced AFTER coalescence.
//!
//! The prompt-path helper `append_replay_entries` does NOT coalesce: each
//! user prompt appends a distinct `User` entry (marked
//! `UserSource::PromptPath`) regardless of any preceding `User` tail, and
//! the helper refuses to merge a `UserSource::ReaderThread` arrival
//! against a `PromptPath` tail or vice versa.

use std::collections::HashMap;

use agentmux::acp::replay::{
    append_replay_entries, coalesce_replay_entries_on_append,
    enforce_replay_buffer_cap_and_maintain_positions,
};
use agentmux::acp::{PendingToolCall, ReplayEntry, UserSource};
use agentmux::transports::ToolCallStatus;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Coalescing helper: User / Agent / Cognition / Update scope
// ---------------------------------------------------------------------------

#[test]
fn within_batch_same_kind_agent_entries_collapse_into_one() {
    let mut buffer: Vec<ReplayEntry> = Vec::new();
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![
            ReplayEntry::Agent {
                lines: vec!["first".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["second".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["third".to_string()],
            },
        ],
    );
    assert_eq!(buffer.len(), 1, "three same-kind entries collapse to one");
    let ReplayEntry::Agent { lines } = &buffer[0] else {
        panic!("expected Agent entry");
    };
    assert_eq!(
        lines,
        &vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ]
    );
}

#[test]
fn tail_of_buffer_same_kind_user_entry_extends_in_place() {
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::User {
        lines: vec!["existing-prompt".to_string()],
        source: UserSource::ReaderThread,
    }];
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![ReplayEntry::User {
            lines: vec!["streaming-delta-1".to_string(), "delta-2".to_string()],
            source: UserSource::ReaderThread,
        }],
    );
    assert_eq!(
        buffer.len(),
        1,
        "tail User should be extended, not appended-to"
    );
    let ReplayEntry::User { lines, .. } = &buffer[0] else {
        panic!("expected User entry");
    };
    assert_eq!(
        lines,
        &vec![
            "existing-prompt".to_string(),
            "streaming-delta-1".to_string(),
            "delta-2".to_string()
        ]
    );
}

#[test]
fn different_kind_adjacency_does_not_merge() {
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::Agent {
        lines: vec!["agent-1".to_string()],
    }];
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![
            ReplayEntry::Cognition {
                lines: vec!["thought".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["agent-2".to_string()],
            },
        ],
    );
    assert_eq!(buffer.len(), 3, "no cross-kind merges");
    assert!(matches!(&buffer[0], ReplayEntry::Agent { .. }));
    assert!(matches!(&buffer[1], ReplayEntry::Cognition { .. }));
    assert!(matches!(&buffer[2], ReplayEntry::Agent { .. }));
}

#[test]
fn invocation_entries_never_merge_across_adjacency() {
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::Invocation {
        call_id: "call-A".to_string(),
        status: ToolCallStatus::Pending,
        invocation: json!({"name": "tool-A"}),
        result: None,
    }];
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![ReplayEntry::Invocation {
            call_id: "call-B".to_string(),
            status: ToolCallStatus::Completed,
            invocation: json!({"name": "tool-B"}),
            result: Some(json!({"ok": true})),
        }],
    );
    assert_eq!(
        buffer.len(),
        2,
        "two distinct tool calls remain two separate entries"
    );
    match &buffer[0] {
        ReplayEntry::Invocation { call_id, .. } => assert_eq!(call_id, "call-A"),
        _ => panic!("expected first entry to be call-A Invocation"),
    }
    match &buffer[1] {
        ReplayEntry::Invocation { call_id, .. } => assert_eq!(call_id, "call-B"),
        _ => panic!("expected second entry to be call-B Invocation"),
    }
}

#[test]
fn update_merging_is_update_kind_aware() {
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::Update {
        update_kind: "permission_requested".to_string(),
        lines: vec!["alpha".to_string()],
    }];
    // Same update_kind -> merges
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![ReplayEntry::Update {
            update_kind: "permission_requested".to_string(),
            lines: vec!["beta".to_string()],
        }],
    );
    assert_eq!(buffer.len(), 1, "matching update_kind merges");
    let ReplayEntry::Update { update_kind, lines } = &buffer[0] else {
        panic!("expected Update entry");
    };
    assert_eq!(update_kind, "permission_requested");
    assert_eq!(lines, &vec!["alpha".to_string(), "beta".to_string()]);

    // Different update_kind -> does not merge; new entry pushed
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![ReplayEntry::Update {
            update_kind: "plan_execution".to_string(),
            lines: vec!["plan-line".to_string()],
        }],
    );
    assert_eq!(
        buffer.len(),
        2,
        "different update_kind produces its own entry"
    );
    match &buffer[1] {
        ReplayEntry::Update { update_kind, .. } => {
            assert_eq!(update_kind, "plan_execution");
        }
        _ => panic!("expected Update entry at index 1"),
    }
}

// ---------------------------------------------------------------------------
// Cap enforcement after coalescence
// ---------------------------------------------------------------------------

#[test]
fn cap_is_enforced_after_coalescence() {
    // Pre-fill 999 entries with same-kind Update tails (each Update has
    // its own update_kind, so they don't co-coalesce; the cap is enforced
    // on entry count, not on update_kind-uniqueness). Push three entries
    // that all fail to merge with the Update tail (different kinds):
    // [Agent, Cognition, Update-new-kind]. None merge; 999 + 3 = 1002,
    // cap drains 2 -> 1000. The coalesce helper is now cap-free; the
    // cap-maintain helper owns the drain.
    let mut buffer: Vec<ReplayEntry> = (0..999)
        .map(|i| ReplayEntry::Update {
            update_kind: format!("fill-{i:03}"),
            lines: vec![],
        })
        .collect();
    assert_eq!(buffer.len(), 999);

    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![
            ReplayEntry::Agent {
                lines: vec!["a".to_string()],
            },
            ReplayEntry::Cognition {
                lines: vec!["c".to_string()],
            },
            ReplayEntry::Update {
                update_kind: "post".to_string(),
                lines: vec!["p".to_string()],
            },
        ],
    );

    let mut pending_calls: std::collections::HashMap<String, PendingToolCall> =
        std::collections::HashMap::new();
    enforce_replay_buffer_cap_and_maintain_positions(&mut buffer, &mut pending_calls);

    assert_eq!(
        buffer.len(),
        1000,
        "cap evicts oldest entries to hold buffer at 1000"
    );
    let last_index = buffer.len() - 1;
    match &buffer[last_index] {
        ReplayEntry::Update { update_kind, .. } => {
            assert_eq!(
                update_kind, "post",
                "most recent entry remains at the tail after cap eviction"
            );
        }
        _ => panic!("expected Update entry at the tail after coalescence"),
    }
    let evicted = &buffer[0];
    match evicted {
        ReplayEntry::Update { update_kind, .. } => {
            assert_eq!(
                update_kind, "fill-002",
                "fill-000 and fill-001 were evicted to maintain the cap"
            );
        }
        _ => panic!("expected an Update entry at index 0 after cap eviction"),
    }
}

#[test]
fn coalescence_reduces_entry_count_before_cap_check() {
    // Pre-fill 996 Updates + 1 Agent tail = 997. Push 4 same-kind Agents:
    // tail extension absorbs all four -> 0 new entries -> buffer holds at
    // 997 (cap not reached). Then push 4 distinct-kind entries: 997 + 4 =
    // 1001 -> cap-maintain drains 1 -> 1000.
    let mut buffer: Vec<ReplayEntry> = (0..996)
        .map(|i| ReplayEntry::Update {
            update_kind: format!("fill-{i:03}"),
            lines: vec![],
        })
        .collect();
    buffer.push(ReplayEntry::Agent {
        lines: vec!["seed".to_string()],
    });
    assert_eq!(buffer.len(), 997);

    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![
            ReplayEntry::Agent {
                lines: vec!["a-1".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["a-2".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["a-3".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["a-4".to_string()],
            },
        ],
    );
    assert_eq!(
        buffer.len(),
        997,
        "four same-kind Agents coalesce into the tail -> buffer holds at 997"
    );

    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![
            ReplayEntry::User {
                lines: vec!["u".to_string()],
                source: UserSource::ReaderThread,
            },
            ReplayEntry::Cognition {
                lines: vec!["c".to_string()],
            },
            ReplayEntry::Update {
                update_kind: "post".to_string(),
                lines: vec!["p".to_string()],
            },
            ReplayEntry::Invocation {
                call_id: "x".to_string(),
                status: ToolCallStatus::Pending,
                invocation: json!({}),
                result: None,
            },
        ],
    );
    let mut pending_calls: std::collections::HashMap<String, PendingToolCall> =
        std::collections::HashMap::new();
    enforce_replay_buffer_cap_and_maintain_positions(&mut buffer, &mut pending_calls);
    assert_eq!(
        buffer.len(),
        1000,
        "coalescence reduced the first batch; cap still holds at 1000"
    );
}

// ---------------------------------------------------------------------------
// Prompt-path non-coalescing append
// ---------------------------------------------------------------------------

#[test]
fn reader_thread_user_does_not_merge_into_prompt_origin_user_tail() {
    // Regression: the buffer tail is a prompt-path User entry
    // (added by AcpStdioClient::prompt before the agent responds).
    // If the upstream ACP server then echoes a user_message_chunk
    // (or session/load replays user content), the parsed User entry
    // is reader-thread. The two must remain separate entries to
    // preserve the per-submission boundary. Pre-fix this would have
    // merged both User lines into one entry because the helper only
    // checked the kind, not the source.
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::User {
        lines: vec!["operator-prompt".to_string()],
        source: UserSource::PromptPath,
    }];
    coalesce_replay_entries_on_append(
        &mut buffer,
        vec![ReplayEntry::User {
            lines: vec!["echoed-from-server".to_string()],
            source: UserSource::ReaderThread,
        }],
    );
    assert_eq!(
        buffer.len(),
        2,
        "cross-source User adjacency must produce two distinct entries"
    );
    match (&buffer[0], &buffer[1]) {
        (
            ReplayEntry::User {
                lines: lines_0,
                source: source_0,
            },
            ReplayEntry::User {
                lines: lines_1,
                source: source_1,
            },
        ) => {
            assert_eq!(lines_0, &vec!["operator-prompt".to_string()]);
            assert_eq!(*source_0, UserSource::PromptPath);
            assert_eq!(lines_1, &vec!["echoed-from-server".to_string()]);
            assert_eq!(*source_1, UserSource::ReaderThread);
        }
        _ => panic!("expected two distinct User entries with sources preserved"),
    }
}

#[test]
fn prompt_path_user_after_reader_thread_user_tail_does_not_merge() {
    // Symmetric case: the buffer tail is a reader-thread User entry
    // (a server emission). A local prompt submission arrives. The
    // prompt-path append helper does not coalesce, so the new entry
    // is pushed regardless of source. This pins the symmetric
    // contract.
    let mut buffer: Vec<ReplayEntry> = vec![ReplayEntry::User {
        lines: vec!["server-user".to_string()],
        source: UserSource::ReaderThread,
    }];
    let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
    append_replay_entries(
        &mut buffer,
        &mut pending_calls,
        vec![ReplayEntry::User {
            lines: vec!["operator-prompt".to_string()],
            source: UserSource::PromptPath,
        }],
    );
    assert_eq!(buffer.len(), 2);
    assert!(matches!(
        buffer[0],
        ReplayEntry::User {
            source: UserSource::ReaderThread,
            ..
        }
    ));
    assert!(matches!(
        buffer[1],
        ReplayEntry::User {
            source: UserSource::PromptPath,
            ..
        }
    ));
}

#[test]
fn prompt_path_append_preserves_user_boundary_on_consecutive_submissions() {
    let mut buffer: Vec<ReplayEntry> = Vec::new();
    let mut pending_calls: HashMap<String, PendingToolCall> = HashMap::new();
    append_replay_entries(
        &mut buffer,
        &mut pending_calls,
        vec![ReplayEntry::User {
            lines: vec!["first-prompt".to_string()],
            source: UserSource::PromptPath,
        }],
    );
    append_replay_entries(
        &mut buffer,
        &mut pending_calls,
        vec![ReplayEntry::User {
            lines: vec!["second-prompt".to_string()],
            source: UserSource::PromptPath,
        }],
    );
    assert_eq!(
        buffer.len(),
        2,
        "two back-to-back prompts through the non-coalescing append remain two entries"
    );
    match (&buffer[0], &buffer[1]) {
        (ReplayEntry::User { lines: lines_0, .. }, ReplayEntry::User { lines: lines_1, .. }) => {
            assert_eq!(lines_0, &vec!["first-prompt".to_string()]);
            assert_eq!(lines_1, &vec!["second-prompt".to_string()]);
        }
        _ => panic!("expected two distinct User entries"),
    }
}

// ---------------------------------------------------------------------------
// session/load replay-history shaping (read-side shaping mirrors the
// session/load replays the reader thread ingests on reconnect)
// ---------------------------------------------------------------------------

fn build_session_load_history_vec() -> Vec<ReplayEntry> {
    // Two assistant turns, each shaped as: User, Agent, Agent, Cognition,
    // Agent, Agent, Cognition (typical multi-chunk shape per turn). The
    // expectation is that the coalescing helper collapses each per-turn
    // same-kind run into one entry per kind, yielding 4 entries per turn
    // (User, Agent, Cognition, Agent at position 4) -> actually 4 distinct
    // entries total when the runs are aligned and merging happens across
    // adjacent same-kinds.
    //
    // More concretely, the per-turn pattern [User, Agent, Agent, Cognition,
    // Agent, Agent, Cognition] coalesces to:
    //   [User, Agent(merged-of-2), Cognition, Agent(merged-of-2), Cognition]
    // i.e., 5 distinct entries per turn. Two turns = 10 entries.
    fn turn() -> Vec<ReplayEntry> {
        vec![
            ReplayEntry::User {
                lines: vec!["user-prompt".to_string()],
                source: UserSource::ReaderThread,
            },
            ReplayEntry::Agent {
                lines: vec!["agent-part-1-line-1".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["agent-part-1-line-2".to_string()],
            },
            ReplayEntry::Cognition {
                lines: vec!["thought-1".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["agent-part-2-line-1".to_string()],
            },
            ReplayEntry::Agent {
                lines: vec!["agent-part-2-line-2".to_string()],
            },
            ReplayEntry::Cognition {
                lines: vec!["thought-2".to_string()],
            },
        ]
    }
    let mut entries = turn();
    entries.extend(turn());
    entries
}

#[test]
fn session_load_shaped_history_vec_coalesces_per_turn() {
    let mut buffer: Vec<ReplayEntry> = Vec::new();
    let history = build_session_load_history_vec();
    assert_eq!(history.len(), 14, "two turns of raw streaming chunks");
    coalesce_replay_entries_on_append(&mut buffer, history);

    // Per turn, [User, Agent(2x), Cognition, Agent(2x), Cognition] merges
    // to [User, Agent-merged, Cognition, Agent-merged, Cognition] = 5
    // entries. Two turns = 10.
    assert_eq!(
        buffer.len(),
        10,
        "buffer holds one entry per kind per turn after coalescence"
    );
    let kinds: Vec<&'static str> = buffer
        .iter()
        .map(|entry| match entry {
            ReplayEntry::User { .. } => "User",
            ReplayEntry::Agent { .. } => "Agent",
            ReplayEntry::Cognition { .. } => "Cognition",
            ReplayEntry::Invocation { .. } => "Invocation",
            ReplayEntry::Update { .. } => "Update",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "User",
            "Agent",
            "Cognition",
            "Agent",
            "Cognition",
            "User",
            "Agent",
            "Cognition",
            "Agent",
            "Cognition",
        ],
        "two turns of coherent kinds, no fragment entries"
    );

    // Spot check: the merged Agent entries carry both source lines.
    let ReplayEntry::Agent { lines } = &buffer[1] else {
        panic!("expected Agent at index 1");
    };
    assert_eq!(
        lines,
        &vec![
            "agent-part-1-line-1".to_string(),
            "agent-part-1-line-2".to_string()
        ]
    );
}

// ---------------------------------------------------------------------------
// Helper to ensure the JSON import is referenced (clippy clean)
// ---------------------------------------------------------------------------

#[test]
fn value_helper_smoke() {
    // Smoke-touch the imports so cargo check on this file remains tight even
    // after later refactors that may inline JSON.
    let _: Value = json!({"ok": true});
}
