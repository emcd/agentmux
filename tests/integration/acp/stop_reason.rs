use agentmux::relay::SendOutcome;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_cancelled_stop_reason_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        stop_reason: "cancelled".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_turn_timeout_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_coder_default_turn_timeout_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(120),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), None);
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_turn_timeout_request_override_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(5_000),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
}

#[test]
fn acp_successful_terminal_stop_reason_is_accepted_for_async_dispatch() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert_eq!(result.reason_code, None);
    assert_eq!(result.reason, None);
}

#[test]
fn acp_first_activity_acceptance_prevents_late_turn_timeout_failure() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let result = send_result(response);
    assert_eq!(result.outcome, SendOutcome::Queued);
    assert_eq!(result.reason_code, None);
}

#[test]
fn acp_async_send_returns_on_dispatch_without_waiting_for_terminal_stop_reason() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 2,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let started_at = Instant::now();
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(5_000),
    );
    let elapsed = started_at.elapsed();
    let result = send_result(response);

    assert_eq!(result.outcome, SendOutcome::Queued);
    assert!(
        elapsed < Duration::from_secs(1),
        "expected async send to return after dispatch, elapsed={elapsed:?}"
    );
}
