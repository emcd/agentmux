use agentmux::relay::{ChatOutcome, ChatStatus};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_cancelled_stop_reason_maps_to_failed_reason_code() {
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
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(result.reason_code.as_deref(), Some("acp_stop_cancelled"));
}

#[test]
fn acp_turn_timeout_maps_to_timeout_outcome_and_reason_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Timeout);
    assert_eq!(result.reason_code.as_deref(), Some("acp_turn_timeout"));
}

#[test]
fn acp_turn_timeout_uses_coder_default_when_request_override_is_absent() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(120),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), None);
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Timeout);
    assert_eq!(result.reason_code.as_deref(), Some("acp_turn_timeout"));
}

#[test]
fn acp_turn_timeout_request_override_takes_precedence_over_coder_default() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_delay_sec: 1,
        coder_turn_timeout_ms: Some(5_000),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(&config_root, &temporary.path().join("tmux.sock"), Some(100));
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Timeout);
    assert_eq!(result.reason_code.as_deref(), Some("acp_turn_timeout"));
}

#[test]
fn acp_successful_terminal_stop_reason_marks_accepted_in_progress_phase() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(result.reason_code, None);
    assert_eq!(result.reason, None);
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
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
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    assert_eq!(result.reason_code, None);
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
}
