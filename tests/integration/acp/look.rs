use agentmux::acp::snapshot_entries_to_plain_lines;
use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::{
    LookFreshness, LookSnapshotPayload, LookSnapshotSource, RelayResponse, SendOutcome,
};
use std::{
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_send_without_startup_fails_when_worker_is_unavailable() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let error = dispatch_send_without_startup_result(&config_root, &tmux_socket)
        .expect_err("ACP send should fail without startup worker");
    assert_eq!(error.code, "runtime_acp_worker_unavailable");
}

#[test]
fn acp_look_without_startup_returns_unavailable_stale_metadata() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let look = dispatch_look_without_startup(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let snapshot = expect_acp_snapshot(look);
    assert!(snapshot.lines.is_empty());
    assert_eq!(snapshot.freshness, LookFreshness::Stale);
    assert_eq!(snapshot.snapshot_source, LookSnapshotSource::None);
    assert_eq!(
        snapshot.stale_reason_code.as_deref(),
        Some("acp_worker_unavailable")
    );
}

#[test]
fn acp_look_returns_oldest_to_newest_session_update_lines() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 3,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    // Settle: wait until the post-coalescence buffer holds both the User
    // prompt and the coalesced Agent entry.
    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
        |lines| lines.len() >= 3 && lines.last().map(String::as_str) == Some("ACP-LINE-3"),
    );
    let snapshot = expect_acp_snapshot(look);
    // The reader-thread same-kind adjacency coalescence rule collapses the 3
    // streaming agent_message_chunk notifications into a single Agent
    // entry; the buffer additionally holds the User prompt entry from
    // `AcpStdioClient::prompt`.
    assert_eq!(
        snapshot.entries.len(),
        2,
        "User prompt + 1 coalesced Agent entry, not 1 + N fragment Agents"
    );
    assert!(matches!(
        snapshot.entries[0],
        agentmux::transports::StructuredEntry::User { .. }
    ));
    match &snapshot.entries[1] {
        agentmux::transports::StructuredEntry::Agent { lines } => {
            assert_eq!(
                lines,
                &vec![
                    "ACP-LINE-1".to_string(),
                    "ACP-LINE-2".to_string(),
                    "ACP-LINE-3".to_string()
                ],
                "the 3 streaming chunks coalesce into one Agent entry's lines"
            );
        }
        other => panic!("expected Agent entry at index 1, got {other:?}"),
    }
    assert_eq!(snapshot.freshness, LookFreshness::Fresh);
    assert_eq!(snapshot.snapshot_source, LookSnapshotSource::LiveBuffer);
    assert_eq!(snapshot.stale_reason_code, None);
}

#[test]
fn acp_look_coalesces_long_streaming_response_into_single_entry() {
    // 1105 streaming chunks would have produced 1105 fragment Agent entries
    // before coalescence, exercising the buffer cap at 1000. With
    // reader-thread same-kind adjacency coalescence in place, the same input coalesces
    // into a single Agent entry whose `lines` carry all 1105 lines; the
    // entry-count cap is never reached (the cap-with-coalescence invariant
    // is exercised at the unit-test layer in tests/unit/acp/replay_coalescence.rs).
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1_105,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(1_000),
        |lines| {
            lines.first().map(String::as_str) == Some("status?")
                && lines.last().map(String::as_str) == Some("ACP-LINE-1105")
        },
    );
    let snapshot = expect_acp_snapshot(look);
    assert_eq!(
        snapshot.entries.len(),
        2,
        "User prompt entry + 1 coalesced Agent entry; the buffer held 2 entries, not 1000+"
    );
    assert_eq!(snapshot.entries_total, 2);
    assert!(matches!(
        snapshot.entries[0],
        agentmux::transports::StructuredEntry::User { .. }
    ));
    let agent_lines = match &snapshot.entries[1] {
        agentmux::transports::StructuredEntry::Agent { lines } => lines.clone(),
        other => panic!("expected Agent entry at index 1, got {other:?}"),
    };
    assert_eq!(
        agent_lines.len(),
        1_105,
        "all 1105 streaming chunks coalesce into one Agent entry's lines"
    );
    assert_eq!(agent_lines.first().map(String::as_str), Some("ACP-LINE-1"));
    assert_eq!(
        agent_lines.last().map(String::as_str),
        Some("ACP-LINE-1105")
    );
}

#[test]
fn acp_look_emits_one_coalesced_invocation_entry_for_tool_call_lifecycle() {
    // End-to-end coverage for the `replace-pending-completed-tool-call-in-place`
    // proposal: an ACP session that streams a single `tool_call` followed by
    // its terminal `tool_call_update` must surface exactly one
    // `StructuredEntry::Invocation` in `look` (status=Completed, with the
    // result payload), not the two separate entries (Pending + Completed)
    // that the pre-`/21` implementation would have produced.
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        tool_call_on_prompt: true,
        tool_call_id: "tc-look-1".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    // Settle on the buffer holding exactly: User prompt + 1 coalesced
    // Invocation entry (status mutated to Completed in place). The
    // post-tool-call-update buffer does NOT have a second Pending entry.
    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
        |lines| {
            lines
                .iter()
                .any(|line| line.starts_with("invocation tc-look-1 Completed"))
        },
    );
    let snapshot = expect_acp_snapshot(look);
    assert_eq!(
        snapshot.entries.len(),
        2,
        "User prompt + 1 coalesced Invocation entry (Pending mutated to Completed in place)"
    );
    assert_eq!(snapshot.entries_total, 2);
    assert!(matches!(
        snapshot.entries[0],
        agentmux::transports::StructuredEntry::User { .. }
    ));
    let invocation = match &snapshot.entries[1] {
        agentmux::transports::StructuredEntry::Invocation {
            call_id,
            status,
            result,
            ..
        } => (call_id.clone(), status.clone(), result.clone()),
        other => panic!("expected Invocation entry at index 1, got {other:?}"),
    };
    assert_eq!(invocation.0, "tc-look-1");
    assert_eq!(
        invocation.1,
        agentmux::transports::ToolCallStatus::Completed,
        "the in-place mutation sets status=Completed"
    );
    let result = invocation
        .2
        .expect("tool_call_update result payload is set on the completed Invocation");
    assert_eq!(
        result["status"], "completed",
        "the stored payload is the tool_call_update notification"
    );
    assert_eq!(
        result["result"]["ok"], true,
        "the payload carries the agent's tool result"
    );
}

#[test]
fn acp_look_offset_walks_backward_through_replay_buffer_with_metadata() {
    // Under reader-thread same-kind adjacency coalescence, a 10-chunk Agent stream
    // coalesces into a single Agent entry; the buffer additionally holds
    // the User prompt entry, so `entries_total == 2`. The offset-walking
    // math still holds on this smaller buffer; the semantics of
    // `entries[total - N - offset .. total - offset]` are unchanged.
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 10,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let full_look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
        |lines| lines.last().map(String::as_str) == Some("ACP-LINE-10"),
    );
    let full = expect_acp_snapshot(full_look);
    let total = full.entries_total;
    assert_eq!(total, 2, "User prompt + 1 coalesced Agent entry");
    assert_eq!(full.returned_entries_count, total);
    assert_eq!(full.entries.len(), total);

    // offset = 0 returns the newest window (tail-N). With a 2-entry buffer
    // and lines=1, the window is [entries[1]] (the coalesced Agent).
    let newest = expect_acp_snapshot(dispatch_look_with_offset(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(1),
        Some(0),
    ));
    assert_eq!(newest.entries_total, total);
    assert_eq!(newest.returned_entries_count, 1);
    assert_eq!(newest.entries, full.entries[total - 1..].to_vec());

    // offset = 1 skips the newest entry and returns the one before it
    // (the User prompt entry).
    let older = expect_acp_snapshot(dispatch_look_with_offset(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(1),
        Some(1),
    ));
    assert_eq!(older.entries_total, total);
    assert_eq!(older.returned_entries_count, 1);
    assert_eq!(older.entries, full.entries[total - 2..total - 1].to_vec());

    // Walking past the start yields an empty window over a still-live buffer.
    let past_start = expect_acp_snapshot(dispatch_look_with_offset(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(1),
        Some(total),
    ));
    assert_eq!(past_start.entries_total, total);
    assert_eq!(past_start.returned_entries_count, 0);
    assert!(past_start.lines.is_empty());
    assert_eq!(past_start.snapshot_source, LookSnapshotSource::LiveBuffer);
}

#[test]
fn acp_look_returns_empty_snapshot_when_no_updates_exist() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let snapshot = expect_acp_snapshot(look);
    assert!(snapshot.lines.is_empty());
    assert_eq!(snapshot.freshness, LookFreshness::Stale);
    assert_eq!(snapshot.snapshot_source, LookSnapshotSource::None);
    assert!(snapshot.stale_reason_code.is_some());
}

#[test]
fn acp_look_reflects_outgoing_user_prompt_before_session_updates_arrive() {
    // Spec scenario (acp-client/spec.md, "Replay buffer updated immediately on
    // outgoing user prompt"): when relay writes a user prompt to ACP stdin,
    // the prompt SHALL be appended to the shared replay buffer as a
    // ReplayEntry::User immediately, so look reflects the submitted message
    // before any session/update response arrives.
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 0,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        None,
        |lines| lines.iter().any(|line| line == "status?"),
    );
    let snapshot = expect_acp_snapshot(look);
    assert!(
        snapshot.entries.iter().any(|entry| matches!(
            entry,
            agentmux::transports::StructuredEntry::User { lines }
                if lines.iter().any(|line| line == "status?")
        )),
        "expected an StructuredEntry::User with the submitted prompt text, got {:?}",
        snapshot.entries,
    );
}

#[test]
fn acp_look_captures_updates_emitted_after_prompt_response() {
    // The stub agent emits 3 chunks before the response and 3 more after a
    // 20ms delay; both batches coalesce against the buffer tail under the
    // reader-thread same-kind adjacency coalescence rule. The buffer ends with a User
    // prompt entry and a single Agent entry whose `lines` carry all 6
    // streamed chunks.
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 3,
        update_after_response: true,
        update_delay_ms: 20,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
        |lines| {
            lines.first().map(String::as_str) == Some("status?")
                && lines.last().map(String::as_str) == Some("ACP-LINE-3")
        },
    );
    let snapshot = expect_acp_snapshot(look);
    assert_eq!(
        snapshot.entries.len(),
        2,
        "User prompt + 1 coalesced Agent entry across pre- and post-response chunks"
    );
    let agent_lines = match &snapshot.entries[1] {
        agentmux::transports::StructuredEntry::Agent { lines } => lines.clone(),
        other => panic!("expected Agent entry at index 1, got {other:?}"),
    };
    assert_eq!(
        agent_lines,
        vec![
            "ACP-LINE-1".to_string(),
            "ACP-LINE-2".to_string(),
            "ACP-LINE-3".to_string()
        ],
        "post-response and pre-response chunks coalesce into the same Agent entry's lines"
    );
}

#[test]
fn acp_look_reuses_persistent_worker_without_one_shot_replay_refresh() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1,
        update_line_prefix: "STALE".to_string(),
        load_replay_count: 2,
        load_replay_line_prefix: "LIVE".to_string(),
        configured_session_id: Some("sess-generated".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(10));
    let snapshot = expect_acp_snapshot(look);
    let snapshot_lines = snapshot.lines;
    assert!(snapshot_lines.iter().any(|line| line == "LIVE-LINE-1"));
    assert!(snapshot_lines.iter().any(|line| line == "LIVE-LINE-2"));
    assert_eq!(snapshot.freshness, LookFreshness::Fresh);
    assert_eq!(snapshot.snapshot_source, LookSnapshotSource::LiveBuffer);
    let requests = read_request_log(&_log_path);
    assert_eq!(request_count_by_method(&requests, "session/load"), 1);
}

/// Reviewer caution (a): a `look` racing a respawn must return clean
/// stale/unavailable or fresh metadata, never panic or read the wrong target's
/// buffer. A respawn clears the published `OutputView` handle and republishes it
/// after startup; `disconnect_on_prompt` drives `Unavailable` -> respawn churn so
/// looks pass through that window. The assertion is that every look yields a
/// well-formed, bounded, target-scoped ACP snapshot.
#[test]
fn acp_look_across_respawn_window_returns_clean_snapshots() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let response = dispatch_send(&config_root, &tmux_socket);
    assert_eq!(send_result(response).outcome, SendOutcome::Queued);

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        // `expect_acp_snapshot` panics if the payload is not a well-formed
        // `StructuredEntriesV1`, so a malformed or wrong-typed response fails the test.
        let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
        let snapshot = expect_acp_snapshot(look);
        assert!(
            snapshot.returned_entries_count <= 5,
            "look window must stay bounded across respawn",
        );
        assert!(
            snapshot.returned_entries_count <= snapshot.entries_total,
            "returned count cannot exceed the total",
        );
        if snapshot.freshness == LookFreshness::Stale {
            assert!(
                snapshot.stale_reason_code.is_some(),
                "a stale snapshot must carry a reason code",
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_look(
    config_root: &ConfigurationRoots,
    tmux_socket: &std::path::Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
    condition: impl Fn(&[String]) -> bool,
) -> RelayResponse {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let look = dispatch_look(
            config_root,
            tmux_socket,
            requester_session,
            target_session,
            lines,
        );
        let snapshot_lines = snapshot_lines_from_look(&look);
        if condition(snapshot_lines.as_slice()) || Instant::now() >= deadline {
            return look;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug)]
struct AcpSnapshotView {
    entries: Vec<agentmux::transports::StructuredEntry>,
    lines: Vec<String>,
    entries_total: usize,
    returned_entries_count: usize,
    freshness: LookFreshness,
    snapshot_source: LookSnapshotSource,
    stale_reason_code: Option<String>,
}

fn expect_acp_snapshot(look: RelayResponse) -> AcpSnapshotView {
    let RelayResponse::Look { snapshot, .. } = look else {
        panic!("expected look response");
    };
    let LookSnapshotPayload::StructuredEntriesV1 {
        snapshot_entries,
        entries_total,
        returned_entries_count,
        freshness,
        snapshot_source,
        stale_reason_code,
        ..
    } = snapshot
    else {
        panic!("expected ACP snapshot payload");
    };
    AcpSnapshotView {
        entries: snapshot_entries.clone(),
        lines: snapshot_entries_to_plain_lines(snapshot_entries.as_slice()),
        entries_total,
        returned_entries_count,
        freshness,
        snapshot_source,
        stale_reason_code,
    }
}

fn snapshot_lines_from_look(look: &RelayResponse) -> Vec<String> {
    let RelayResponse::Look { snapshot, .. } = look else {
        panic!("expected look response");
    };
    match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines.clone(),
        LookSnapshotPayload::StructuredEntriesV1 {
            snapshot_entries, ..
        } => snapshot_entries_to_plain_lines(snapshot_entries.as_slice()),
    }
}
