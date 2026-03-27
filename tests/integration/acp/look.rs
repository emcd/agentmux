use agentmux::relay::{ChatOutcome, ChatStatus, RelayResponse};
use std::{
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

use super::helpers::*;

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
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert_eq!(
        snapshot_lines,
        vec!["ACP-LINE-1", "ACP-LINE-2", "ACP-LINE-3"]
    );
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
        |lines| lines.len() == 1_000,
    );
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
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
    let RelayResponse::Look {
        snapshot_lines: tail_lines,
        ..
    } = tail
    else {
        panic!("expected look response");
    };
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
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert!(snapshot_lines.is_empty());
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
    let RelayResponse::Look { snapshot_lines, .. } = look else {
        panic!("expected look response");
    };
    assert_eq!(
        snapshot_lines,
        vec!["ACP-LINE-1", "ACP-LINE-2", "ACP-LINE-3"]
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
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let look = dispatch_look(
            config_root,
            tmux_socket,
            requester_session,
            target_session,
            lines,
        );
        let RelayResponse::Look { snapshot_lines, .. } = &look else {
            panic!("expected look response");
        };
        if condition(snapshot_lines) || Instant::now() >= deadline {
            return look;
        }
        thread::sleep(Duration::from_millis(20));
    }
}
