//! Parser-level coverage for the tool-call lifecycle replace-by-key path
//! (per `replace-pending-completed-tool-call-in-place` proposal).
//!
//! The parser is buffer-aware: it ingests notifications into a live
//! `Vec<ReplayEntry>` and maintains the `pending_calls` map keyed by
//! `call_id`. On a terminal `tool_call_update`, the recorded
//! `PendingToolCall::buffer_position` lets the parser mutate the existing
//! buffer entry in place rather than appending a second Invocation entry.
//!
//! The cap-aware behavior is exercised in tests 2.4 and 2.5 against the
//! `enforce_replay_buffer_cap_and_maintain_positions` helper: the parser
//! invokes that helper once at the end of the ingest pass so cap drain
//! and recorded-position adjustment happen atomically.

use std::collections::HashMap;

use agentmux::acp::replay::parse_replay_entries_from_params;
use agentmux::acp::{PendingToolCall, REPLAY_BUFFER_MAX_ENTRIES, ReplayEntry};
use agentmux::transports::ToolCallStatus;
use serde_json::{Value, json};

fn tool_call_params(call_id: &str) -> Value {
    json!({
        "sessionId": "sess_1",
        "update": [{
            "sessionUpdate": "tool_call",
            "toolCallId": call_id,
            "tool": "search",
            "args": {"q": "test"}
        }]
    })
}

fn tool_call_update_params(call_id: &str, result_payload: Value) -> Value {
    json!({
        "sessionId": "sess_1",
        "update": [{
            "sessionUpdate": "tool_call_update",
            "toolCallId": call_id,
            "result": result_payload
        }]
    })
}

fn call_id_of(entry: &ReplayEntry) -> &str {
    let ReplayEntry::Invocation { call_id, .. } = entry else {
        panic!("expected Invocation entry, got: {entry:?}");
    };
    call_id.as_str()
}

fn invocation_status(entry: &ReplayEntry) -> ToolCallStatus {
    let ReplayEntry::Invocation { status, .. } = entry else {
        panic!("expected Invocation entry, got: {entry:?}");
    };
    status.clone()
}

fn invocation_result(entry: &ReplayEntry) -> Option<Value> {
    let ReplayEntry::Invocation { result, .. } = entry else {
        panic!("expected Invocation entry, got: {entry:?}");
    };
    result.clone()
}

// 2.1 Unit test (parser): `tool_call(A)` then `tool_call_update(A)` with
// terminal `status="completed"`. Buffer holds exactly one
// `ReplayEntry::Invocation` with `status="completed"` and the update
// payload; no second entry appears.
#[test]
fn parser_replaces_pending_with_completed_in_place() {
    let mut pending = HashMap::new();
    let mut buffer = Vec::new();

    parse_replay_entries_from_params(&tool_call_params("call-A"), &mut pending, &mut buffer);
    assert_eq!(buffer.len(), 1, "Pending A pushes one entry");
    assert_eq!(call_id_of(&buffer[0]), "call-A");
    assert_eq!(invocation_status(&buffer[0]), ToolCallStatus::Pending);
    assert!(invocation_result(&buffer[0]).is_none());

    let update_params = tool_call_update_params("call-A", json!({"ok": true}));
    parse_replay_entries_from_params(&update_params, &mut pending, &mut buffer);

    assert_eq!(
        buffer.len(),
        1,
        "Completed A mutates the existing entry in place; no second entry"
    );
    assert_eq!(call_id_of(&buffer[0]), "call-A");
    assert_eq!(
        invocation_status(&buffer[0]),
        ToolCallStatus::Completed,
        "the in-place mutation sets status=Completed"
    );
    // The parser stores the entire `tool_call_update` notification as the
    // Invocation's `result`, mirroring the pre-/21 behavior. The shape
    // pin (presence of `result` and the call_id) is sufficient for /21:
    // the in-place mutation is what the contract is about, not payload
    // extraction.
    let result = invocation_result(&buffer[0]).expect("result populated");
    assert_eq!(result["sessionUpdate"], "tool_call_update");
    assert_eq!(result["toolCallId"], "call-A");
    assert_eq!(result["result"]["ok"], true);
    assert!(
        pending.is_empty(),
        "the pending entry is removed once Completed in place"
    );
}

// 2.2 Unit test (parser): two tool_calls with out-of-order updates
// (`tool_call(A)`, `tool_call(B)`, `tool_call_update(B)`,
// `tool_call_update(A)`). Buffer holds two Invocation entries, each
// carrying its own update payload -- no cross-contamination.
#[test]
fn parser_replaces_pending_by_call_id_with_no_cross_contamination() {
    let mut pending = HashMap::new();
    let mut buffer = Vec::new();

    parse_replay_entries_from_params(&tool_call_params("call-A"), &mut pending, &mut buffer);
    parse_replay_entries_from_params(&tool_call_params("call-B"), &mut pending, &mut buffer);
    assert_eq!(buffer.len(), 2);
    assert_eq!(call_id_of(&buffer[0]), "call-A");
    assert_eq!(call_id_of(&buffer[1]), "call-B");

    let update_b = tool_call_update_params("call-B", json!({"b": 1}));
    parse_replay_entries_from_params(&update_b, &mut pending, &mut buffer);
    assert_eq!(buffer.len(), 2, "B's update mutates B's entry in place");
    assert_eq!(invocation_status(&buffer[0]), ToolCallStatus::Pending);
    assert_eq!(invocation_status(&buffer[1]), ToolCallStatus::Completed);
    let result_b = invocation_result(&buffer[1]).expect("B result populated");
    assert_eq!(result_b["result"]["b"], 1);
    assert!(pending.contains_key("call-A"));
    assert!(!pending.contains_key("call-B"));

    let update_a = tool_call_update_params("call-A", json!({"a": 2}));
    parse_replay_entries_from_params(&update_a, &mut pending, &mut buffer);
    assert_eq!(
        buffer.len(),
        2,
        "A's update mutates A's entry in place; still no second entries"
    );
    assert_eq!(invocation_status(&buffer[0]), ToolCallStatus::Completed);
    assert_eq!(invocation_status(&buffer[1]), ToolCallStatus::Completed);
    let result_a = invocation_result(&buffer[0]).expect("A result populated");
    assert_eq!(result_a["result"]["a"], 2);
    let result_b_post = invocation_result(&buffer[1]).expect("B result still populated");
    assert_eq!(result_b_post["result"]["b"], 1);
    assert!(
        pending.is_empty(),
        "both pendings are removed once Completed in place"
    );
}

// 2.3 Unit test (parser): a terminal `tool_call_update(X)` with no prior
// `tool_call(X)`. Buffer holds one Invocation entry with
// `status="completed"` and the update payload -- the replay-baseline
// affordance.
#[test]
fn parser_terminal_update_without_prior_call_pushes_completed_orphan() {
    let mut pending = HashMap::new();
    let mut buffer = Vec::new();

    let update_params = tool_call_update_params("call-orphan", json!({"ok": false}));
    parse_replay_entries_from_params(&update_params, &mut pending, &mut buffer);

    assert_eq!(buffer.len(), 1);
    assert_eq!(call_id_of(&buffer[0]), "call-orphan");
    assert_eq!(invocation_status(&buffer[0]), ToolCallStatus::Completed);
    let result = invocation_result(&buffer[0]).expect("orphan result populated");
    assert_eq!(result["result"]["ok"], false);
    assert!(
        pending.is_empty(),
        "an orphan tool_call_update does not touch pending_calls"
    );
}

// 2.4 Unit test (parser + cap-maintain helper): cap-eviction position
// shift. Pre-fill the buffer with 995 distinct `Update` entries (each
// carrying a unique `update_kind`, so coalescence does not absorb them).
// Invoke the parser with a `tool_call` notification; record the resulting
// `pending_calls[call_id].buffer_position == 995`. Then invoke the
// parser with five distinct `Update` entries (each with a distinct
// `sessionUpdate` so they don't merge); this trips the cap
// (995 + 1 + 5 = 1001; the cap-maintain helper drains 1 and shifts the
// recorded position to 994). Verify
// `pending_calls[call_id].buffer_position == 994` and that
// `buffer[994]` is still the same Pending Invocation (matched by
// `call_id`). A subsequent `tool_call_update(call_id,
// status="completed")` mutates `buffer[994]` in place to
// `status="completed"` and removes the entry from `pending_calls`.
#[test]
fn parser_with_cap_shift_keeps_recorded_position_consistent() {
    let mut pending = HashMap::new();
    let mut buffer: Vec<ReplayEntry> = (0..995)
        .map(|i| ReplayEntry::Update {
            update_kind: format!("fill-{i:03}"),
            lines: vec![],
        })
        .collect();

    parse_replay_entries_from_params(&tool_call_params("call-A"), &mut pending, &mut buffer);
    assert_eq!(buffer.len(), 996);
    assert_eq!(pending.get("call-A").map(|p| p.buffer_position), Some(995));

    // Five distinct sessionUpdate kinds so the Update-merging rule
    // (same-kind adjacency merges) does not absorb them into one entry.
    let filler: Vec<Value> = (0..5)
        .map(|i| json!({"sessionUpdate": format!("post-{i}")}))
        .collect();
    let params = json!({"sessionId": "sess_1", "update": filler});
    parse_replay_entries_from_params(&params, &mut pending, &mut buffer);

    assert_eq!(
        buffer.len(),
        1000,
        "cap-maintain drains exactly 1 entry to hold the cap"
    );
    assert_eq!(
        pending.get("call-A").map(|p| p.buffer_position),
        Some(994),
        "the recorded position shifts down by the drain count"
    );
    assert_eq!(
        call_id_of(&buffer[994]),
        "call-A",
        "buffer[994] is still the same Pending Invocation"
    );

    let update_params = tool_call_update_params("call-A", json!({"ok": true}));
    parse_replay_entries_from_params(&update_params, &mut pending, &mut buffer);
    assert_eq!(
        buffer.len(),
        1000,
        "Completed mutates in place; no new entry is appended"
    );
    assert_eq!(invocation_status(&buffer[994]), ToolCallStatus::Completed);
    assert!(invocation_result(&buffer[994]).is_some());
    assert!(
        !pending.contains_key("call-A"),
        "pending is removed after the in-place Completed mutation"
    );
}

// 2.5 Unit test (parser + cap-maintain helper): Pending evicted before
// completion. Empty buffer; push a `tool_call` (Pending at position 0);
// push 1001 distinct `Update` entries in one parser invocation so the
// cap-maintain helper drains 1 and removes the Pending. Verify
// `pending_calls` no longer references the `call_id`. Push a terminal
// `tool_call_update(call_id)` for the same call_id; the parser falls
// through to the replay-baseline path (Task 1.6), so the buffer ends up
// with one new Invocation entry carrying the update payload (and
// `pending_calls` remains empty).
#[test]
fn parser_falls_through_when_pending_was_evicted_by_cap() {
    let mut pending = HashMap::new();
    let mut buffer = Vec::new();

    parse_replay_entries_from_params(&tool_call_params("call-A"), &mut pending, &mut buffer);
    assert_eq!(buffer.len(), 1);
    assert_eq!(pending.get("call-A").map(|p| p.buffer_position), Some(0));

    // 1001 distinct sessionUpdate kinds so the Update-merging rule does
    // not absorb them. After the cap-maintain helper drains to 1000, the
    // original Pending at position 0 is gone (evicted by the drain).
    let filler: Vec<Value> = (0..1001)
        .map(|i| json!({"sessionUpdate": format!("flood-{i:04}")}))
        .collect();
    let params = json!({"sessionId": "sess_1", "update": filler});
    parse_replay_entries_from_params(&params, &mut pending, &mut buffer);

    assert_eq!(
        buffer.len(),
        1000,
        "cap-maintain drains exactly 1 entry to hold the cap"
    );
    assert!(
        !pending.contains_key("call-A"),
        "Pending was evicted by the cap and removed from pending_calls"
    );

    let update_params = tool_call_update_params("call-A", json!({"ok": true}));
    parse_replay_entries_from_params(&update_params, &mut pending, &mut buffer);

    assert_eq!(
        buffer.len(),
        1000,
        "the cap-maintain helper still drains to 1000 after the orphan push"
    );
    let last = buffer.last().expect("at least one entry");
    assert_eq!(call_id_of(last), "call-A");
    assert_eq!(invocation_status(last), ToolCallStatus::Completed);
    assert!(invocation_result(last).is_some());
    assert!(pending.is_empty());
}

#[test]
fn parser_buffer_aware_test_re_export_is_callable_with_explicit_buffer() {
    // Smoke test for the test re-export signature in `src/acp/mod.rs`.
    let mut pending: HashMap<String, PendingToolCall> = HashMap::new();
    let mut buffer = Vec::new();
    parse_replay_entries_from_params(&tool_call_params("call-X"), &mut pending, &mut buffer);
    assert_eq!(pending.len(), 1);
    assert_eq!(buffer.len(), 1);
}

// A prompt-path append that overflows the buffer must drain the cap
// through the same position-maintenance path the reader-thread parser
// uses, so a later `tool_call_update` does not mutate the wrong buffer
// entry or panic on out-of-bounds indexing. Without shared
// `pending_tool_calls` and a unified cap-maintain helper, a prompt-path
// overflow leaves stale `buffer_position` values in the pending map
// because the drain would otherwise happen in one place while the
// position maintenance happens in another.
#[test]
fn prompt_path_cap_drain_maintains_pending_position_and_completes_in_place() {
    use agentmux::acp::replay::append_replay_entries;

    let mut buffer: Vec<ReplayEntry> = (0..REPLAY_BUFFER_MAX_ENTRIES)
        .map(|i| ReplayEntry::Update {
            update_kind: format!("fill-{i:03}"),
            lines: vec![],
        })
        .collect();

    // Reader-thread ingestion records a Pending tool call near the tail.
    let mut pending = HashMap::new();
    parse_replay_entries_from_params(&tool_call_params("call-A"), &mut pending, &mut buffer);
    assert_eq!(pending.get("call-A").map(|p| p.buffer_position), Some(999));

    // A prompt-path append overflows the cap. The cap-maintain helper
    // must drop the frontmost entry, decrement the recorded position,
    // and the Pending entry must still reference a valid Invocation.
    let overflow = vec![ReplayEntry::User {
        lines: vec!["next-prompt".to_string()],
        source: agentmux::acp::UserSource::PromptPath,
    }];
    append_replay_entries(&mut buffer, &mut pending, overflow);

    assert_eq!(
        buffer.len(),
        REPLAY_BUFFER_MAX_ENTRIES,
        "cap-maintain keeps the buffer at the cap"
    );
    assert_eq!(
        pending.get("call-A").map(|p| p.buffer_position),
        Some(998),
        "the recorded position shifts down by the drain count"
    );
    assert_eq!(
        call_id_of(&buffer[998]),
        "call-A",
        "buffer[998] is still the Pending Invocation"
    );

    // A later tool_call_update mutates the Pending in place; without
    // position maintenance this would have indexed the wrong entry or
    // panicked.
    let update_params = tool_call_update_params("call-A", json!({"ok": true}));
    parse_replay_entries_from_params(&update_params, &mut pending, &mut buffer);

    assert_eq!(
        buffer.len(),
        REPLAY_BUFFER_MAX_ENTRIES,
        "Completed mutates in place; no new entry appended"
    );
    assert_eq!(invocation_status(&buffer[998]), ToolCallStatus::Completed);
    assert!(invocation_result(&buffer[998]).is_some());
    assert!(
        !pending.contains_key("call-A"),
        "pending is removed after the in-place Completed mutation"
    );
}

// Parser wire-order preservation: an `update` array that mixes
// non-tool entries with tool-call entries must land in the buffer in
// the order the wire sent them. The pre-fix parser batched non-tool
// entries and pushed tool calls in place, so a wire-order sequence
// like [agent_message_chunk, tool_call] produced a buffer with the
// Invocation BEFORE the Agent entry.
#[test]
fn parser_preserves_wire_order_for_mixed_non_tool_and_tool_updates() {
    let mut pending = HashMap::new();
    let mut buffer = Vec::new();

    let params = json!({
        "sessionId": "sess_1",
        "update": [
            {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "before-tool"}},
            {"sessionUpdate": "tool_call", "toolCallId": "call-A", "title": "stub-tool", "kind": "exec"},
            {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "after-tool"}},
            {"sessionUpdate": "tool_call_update", "toolCallId": "call-A", "status": "completed", "result": {"ok": true}},
        ]
    });
    parse_replay_entries_from_params(&params, &mut pending, &mut buffer);

    assert_eq!(buffer.len(), 3, "wire-order: Agent, Invocation, Agent");

    let kind_at = |i: usize| match &buffer[i] {
        ReplayEntry::Agent { .. } => "Agent",
        ReplayEntry::Invocation { status, .. } => match status {
            ToolCallStatus::Pending => "PendingInvocation",
            ToolCallStatus::Completed => "CompletedInvocation",
        },
        other => panic!("unexpected entry kind at index {i}: {other:?}"),
    };
    assert_eq!(kind_at(0), "Agent", "Agent arrives first (wire order)");
    assert_eq!(
        kind_at(1),
        "CompletedInvocation",
        "tool_call landed at index 1 and the tool_call_update mutated it in place"
    );
    assert_eq!(kind_at(2), "Agent", "Agent after the tool_call lands after");

    // The Agent at index 0 carries only the pre-tool line.
    let ReplayEntry::Agent { lines } = &buffer[0] else {
        panic!("expected Agent at index 0");
    };
    assert_eq!(lines, &vec!["before-tool".to_string()]);
    // The Agent at index 2 carries only the post-tool line (no
    // cross-buffer coalescence with the pre-tool Agent because the
    // Invocation sits between them).
    let ReplayEntry::Agent { lines } = &buffer[2] else {
        panic!("expected Agent at index 2");
    };
    assert_eq!(lines, &vec!["after-tool".to_string()]);
    assert!(pending.is_empty());
}
