use agentmux::relay::{ChatOutcome, ChatStatus};
use serde_json::Value;
use std::{
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_next_send_recovers_after_connection_closed_failure() {
    let temporary = TempDir::new().expect("temporary");
    let failing = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &failing);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (first_status, first_result) = chat_result(first);
    assert_eq!(first_status, ChatStatus::Success);
    assert_eq!(first_result.outcome, ChatOutcome::Delivered);
    assert_eq!(
        first_result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "unavailable",
            Duration::from_secs(1)
        ),
        "worker_state did not converge to unavailable"
    );

    let recovered = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &recovered);
    let second = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (second_status, second_result) = chat_result(second);
    assert_eq!(second_status, ChatStatus::Failure);
    assert_eq!(second_result.outcome, ChatOutcome::Failed);
    assert_eq!(
        second_result.reason_code.as_deref(),
        Some("runtime_acp_worker_unavailable")
    );
}

#[test]
fn acp_next_send_recovers_after_post_accept_disconnect() {
    let temporary = TempDir::new().expect("temporary");
    let failing = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &failing);
    let first = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (first_status, first_result) = chat_result(first);
    assert_eq!(first_status, ChatStatus::Success);
    assert_eq!(first_result.outcome, ChatOutcome::Delivered);
    assert_eq!(
        first_result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
    assert!(
        wait_for_worker_state(
            temporary.path(),
            "bravo",
            "unavailable",
            Duration::from_secs(1)
        ),
        "worker_state did not converge to unavailable"
    );

    let recovered = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &recovered);
    let second = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (second_status, second_result) = chat_result(second);
    assert_eq!(second_status, ChatStatus::Failure);
    assert_eq!(second_result.outcome, ChatOutcome::Failed);
    assert_eq!(
        second_result.reason_code.as_deref(),
        Some("runtime_acp_worker_unavailable")
    );
}

fn wait_for_worker_state(
    root: &std::path::Path,
    target_session: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if read_worker_state(root, target_session).as_deref() == Some(expected) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}
