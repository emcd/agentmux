use agentmux::acp::{ReplayEntry, ToolCallStatus, parse_replay_entries_for_test};
use std::collections::HashMap;

#[test]
fn invocation_coalescing_pending_to_completed() {
    let mut pending = HashMap::new();

    let tool_call = serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call_1",
        "tool": "search",
        "args": {"q": "test"}
    });
    let params = serde_json::json!({"sessionId": "sess_1", "update": [tool_call]});
    let entries = parse_replay_entries_for_test(&params, &mut pending);

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    let (call_id, status, result) = match entry {
        ReplayEntry::Invocation {
            call_id,
            status,
            result,
            ..
        } => (call_id.clone(), status.clone(), result.clone()),
        _ => panic!("expected Invocation"),
    };
    assert_eq!(call_id, "call_1");
    assert_eq!(status, ToolCallStatus::Pending);
    assert!(result.is_none());

    let tool_result = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call_1",
        "result": {"ok": true}
    });
    let params = serde_json::json!({"sessionId": "sess_1", "update": [tool_result]});
    let entries = parse_replay_entries_for_test(&params, &mut pending);

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    let (call_id, status, result) = match entry {
        ReplayEntry::Invocation {
            call_id,
            status,
            result,
            ..
        } => (call_id.clone(), status.clone(), result.clone()),
        _ => panic!("expected Invocation"),
    };
    assert_eq!(call_id, "call_1");
    assert_eq!(status, ToolCallStatus::Completed);
    assert!(result.is_some());
}

#[test]
fn invocation_orphan_result_creates_standalone_entry() {
    let mut pending = HashMap::new();

    let tool_result = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call_orphan",
        "result": {"ok": false}
    });
    let params = serde_json::json!({"sessionId": "sess_1", "update": [tool_result]});
    let entries = parse_replay_entries_for_test(&params, &mut pending);

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    let (call_id, status, result) = match entry {
        ReplayEntry::Invocation {
            call_id,
            status,
            result,
            ..
        } => (call_id.clone(), status.clone(), result.clone()),
        _ => panic!("expected Invocation"),
    };
    assert_eq!(call_id, "call_orphan");
    assert_eq!(status, ToolCallStatus::Completed);
    assert!(result.is_some());
}

#[test]
fn tool_call_without_tool_call_id_is_dropped() {
    let mut pending = HashMap::new();

    let tool_call = serde_json::json!({
        "sessionUpdate": "tool_call",
        "tool": "search",
    });
    let params = serde_json::json!({"sessionId": "sess_1", "update": [tool_call]});
    let entries = parse_replay_entries_for_test(&params, &mut pending);
    assert!(entries.is_empty());
    assert!(pending.is_empty());

    let tool_update = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "result": {"ok": false},
    });
    let params = serde_json::json!({"sessionId": "sess_1", "update": [tool_update]});
    let entries = parse_replay_entries_for_test(&params, &mut pending);
    assert!(entries.is_empty());
}
