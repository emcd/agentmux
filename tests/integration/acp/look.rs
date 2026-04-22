use agentmux::acp::snapshot_entries_to_plain_lines;
use agentmux::relay::{
    AcpLookFreshness, AcpLookSnapshotSource, ChatOutcome, ChatStatus, LookSnapshotPayload,
    RelayResponse,
};
use std::{
    fs, thread,
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

    let error = dispatch_send_without_startup_result(
        &config_root,
        &tmux_socket,
        Some(1_000),
        ChatDeliveryMode::Sync,
    )
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
    assert_eq!(snapshot.freshness, AcpLookFreshness::Stale);
    assert_eq!(snapshot.snapshot_source, AcpLookSnapshotSource::None);
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
    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(3),
        |lines| lines.len() == 3,
    );
    let snapshot = expect_acp_snapshot(look);
    assert!(snapshot.entries.iter().all(|entry| {
        matches!(
            entry,
            agentmux::acp::AcpSnapshotEntry::Update { update_kind, .. } if update_kind == "text"
        )
    }));
    assert_eq!(
        snapshot.lines,
        vec!["ACP-LINE-1", "ACP-LINE-2", "ACP-LINE-3"]
    );
    assert_eq!(snapshot.freshness, AcpLookFreshness::Fresh);
    assert_eq!(snapshot.snapshot_source, AcpLookSnapshotSource::LiveBuffer);
    assert_eq!(snapshot.stale_reason_code, None);
}

#[test]
fn acp_look_enforces_bounded_retention_and_tail_selection() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1_105,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket, Some(2_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(1_000),
        |lines| {
            lines.len() == 1_000
                && lines.first().map(String::as_str) == Some("ACP-LINE-106")
                && lines.last().map(String::as_str) == Some("ACP-LINE-1105")
        },
    );
    let snapshot = expect_acp_snapshot(look);
    let snapshot_lines = snapshot.lines;
    assert_eq!(snapshot_lines.len(), 1_000);
    assert_eq!(
        snapshot_lines.first().map(String::as_str),
        Some("ACP-LINE-106")
    );
    assert_eq!(
        snapshot_lines.last().map(String::as_str),
        Some("ACP-LINE-1105")
    );

    let tail = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let tail_lines = expect_acp_snapshot(tail).lines;
    assert_eq!(
        tail_lines,
        vec![
            "ACP-LINE-1101".to_string(),
            "ACP-LINE-1102".to_string(),
            "ACP-LINE-1103".to_string(),
            "ACP-LINE-1104".to_string(),
            "ACP-LINE-1105".to_string(),
        ]
    );
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
    assert_eq!(snapshot.freshness, AcpLookFreshness::Stale);
    assert_eq!(snapshot.snapshot_source, AcpLookSnapshotSource::None);
    assert!(snapshot.stale_reason_code.is_some());
}

#[test]
fn acp_look_captures_updates_emitted_after_prompt_response() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 3,
        update_after_response: true,
        update_delay_ms: 20,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = wait_for_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(3),
        |lines| lines.len() == 3,
    );
    let snapshot_lines = expect_acp_snapshot(look).lines;
    assert_eq!(
        snapshot_lines,
        vec!["ACP-LINE-1", "ACP-LINE-2", "ACP-LINE-3"]
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
    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(10));
    let snapshot = expect_acp_snapshot(look);
    let snapshot_lines = snapshot.lines;
    assert!(snapshot_lines.iter().any(|line| line == "LIVE-LINE-1"));
    assert!(snapshot_lines.iter().any(|line| line == "LIVE-LINE-2"));
    assert_eq!(snapshot.freshness, AcpLookFreshness::Fresh);
    assert_eq!(snapshot.snapshot_source, AcpLookSnapshotSource::LiveBuffer);
    let requests = read_request_log(&_log_path);
    assert_eq!(request_count_by_method(&requests, "session/load"), 1);
}

#[test]
fn acp_look_replaces_legacy_flattened_baseline_after_structured_load() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 0,
        load_replay_count: 2,
        load_replay_line_prefix: "LIVE".to_string(),
        configured_session_id: Some("sess-generated".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let legacy_state_path = persisted_state_path(temporary.path(), "bravo");
    fs::create_dir_all(
        legacy_state_path
            .parent()
            .expect("legacy state parent directory"),
    )
    .expect("create legacy state directory");
    fs::write(
        &legacy_state_path,
        serde_json::json!({
            "schema_version": 1,
            "acp_session_id": "sess-generated",
            "worker_state": "available",
            "snapshot_lines": ["LEGACY-LINE-1", "LEGACY-LINE-2"]
        })
        .to_string(),
    )
    .expect("write legacy ACP state");

    let pre_load_snapshot = expect_acp_snapshot(dispatch_look_without_startup(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
    ));
    assert!(pre_load_snapshot.lines.is_empty());
    assert_eq!(pre_load_snapshot.freshness, AcpLookFreshness::Stale);

    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let snapshot = expect_acp_snapshot(dispatch_look(
        &config_root,
        &tmux_socket,
        "bravo",
        "bravo",
        Some(10),
    ));
    assert!(snapshot.lines.iter().any(|line| line == "LIVE-LINE-1"));
    assert!(snapshot.lines.iter().any(|line| line == "LIVE-LINE-2"));
    assert!(!snapshot.lines.iter().any(|line| line == "LEGACY-LINE-1"));
    assert!(!snapshot.lines.iter().any(|line| line == "LEGACY-LINE-2"));
}

#[test]
fn acp_look_marks_snapshot_stale_when_updates_are_stalled() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");
    let response = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    thread::sleep(Duration::from_millis(5_200));

    let look = dispatch_look(&config_root, &tmux_socket, "bravo", "bravo", Some(5));
    let snapshot = expect_acp_snapshot(look);
    assert_eq!(snapshot.freshness, AcpLookFreshness::Stale);
    assert_eq!(snapshot.snapshot_source, AcpLookSnapshotSource::LiveBuffer);
    assert_eq!(
        snapshot.stale_reason_code.as_deref(),
        Some("acp_stream_stalled")
    );
}

fn wait_for_look(
    config_root: &std::path::Path,
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
    entries: Vec<agentmux::acp::AcpSnapshotEntry>,
    lines: Vec<String>,
    freshness: AcpLookFreshness,
    snapshot_source: AcpLookSnapshotSource,
    stale_reason_code: Option<String>,
}

fn expect_acp_snapshot(look: RelayResponse) -> AcpSnapshotView {
    let RelayResponse::Look { snapshot, .. } = look else {
        panic!("expected look response");
    };
    let LookSnapshotPayload::AcpEntriesV1 {
        snapshot_entries,
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
        LookSnapshotPayload::AcpEntriesV1 {
            snapshot_entries, ..
        } => snapshot_entries_to_plain_lines(snapshot_entries.as_slice()),
    }
}
