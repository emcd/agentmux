use std::fs;

use agentmux::relay::{ChatOutcome, ChatStatus};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_send_selects_session_new_without_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let state_path = persisted_state_path(temporary.path(), "bravo");
    assert!(
        state_path.is_file(),
        "missing state file: {}",
        state_path.display()
    );
    let persisted: Value = serde_json::from_str(
        fs::read_to_string(&state_path)
            .expect("read state file")
            .as_str(),
    )
    .expect("parse state json");
    assert_eq!(persisted["schema_version"], 1);
    assert_eq!(persisted["acp_session_id"], "sess-generated");

    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/new\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_sync_send_reuses_persistent_worker_across_requests() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let first = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let second = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (first_status, first_result) = chat_result(first);
    let (second_status, second_result) = chat_result(second);
    assert_eq!(first_status, ChatStatus::Success);
    assert_eq!(first_result.outcome, ChatOutcome::Delivered);
    assert_eq!(second_status, ChatStatus::Success);
    assert_eq!(second_result.outcome, ChatOutcome::Delivered);

    let requests = read_request_log(log_path.as_path());
    assert_eq!(request_count_by_method(&requests, "initialize"), 1);
    assert_eq!(request_count_by_method(&requests, "session/new"), 1);
    assert_eq!(request_count_by_method(&requests, "session/prompt"), 2);
}

#[test]
fn acp_initialize_request_uses_protocol_version_integer_and_client_version() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let requests = read_request_log(log_path.as_path());
    let initialize = request_by_method(requests.as_slice(), "initialize");
    let params = initialize.get("params").expect("initialize params object");

    assert_eq!(params["protocolVersion"], 1);
    assert_eq!(params["clientInfo"]["name"], "agentmux-relay");
    assert!(
        params["clientInfo"]["version"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(params["clientInfo"].get("title").is_none());
    assert_eq!(params["clientCapabilities"]["terminal"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], false);
    assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
}

#[test]
fn acp_session_setup_requests_include_mcp_servers_array() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions::default();
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let requests = read_request_log(log_path.as_path());
    let session_new = request_by_method(requests.as_slice(), "session/new");
    assert_eq!(
        session_new["params"]["mcpServers"],
        Value::Array(Vec::new())
    );

    let second_temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        configured_session_id: Some("sess-configured".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(second_temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &second_temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);

    let requests = read_request_log(log_path.as_path());
    let session_load = request_by_method(requests.as_slice(), "session/load");
    assert_eq!(
        session_load["params"]["mcpServers"],
        Value::Array(Vec::new())
    );
}

#[test]
fn acp_send_uses_persisted_session_id_when_config_id_is_absent() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let tmux_socket = temporary.path().join("tmux.sock");

    let first = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let second = dispatch_send(&config_root, &tmux_socket, Some(1_000));
    let (first_status, first_result) = chat_result(first);
    let (second_status, second_result) = chat_result(second);
    assert_eq!(first_status, ChatStatus::Success);
    assert_eq!(second_status, ChatStatus::Success);
    assert_eq!(first_result.outcome, ChatOutcome::Delivered);
    assert_eq!(second_result.outcome, ChatOutcome::Delivered);

    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert_eq!(
        log.matches("\"method\":\"session/new\"").count(),
        1,
        "log={log}"
    );
    assert_eq!(
        log.matches("\"method\":\"session/load\"").count(),
        1,
        "log={log}"
    );
}

#[test]
fn acp_send_selects_session_load_with_configured_coder_session_id() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Success);
    assert_eq!(result.outcome, ChatOutcome::Delivered);
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_load_failure_does_not_fallback_to_session_new() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_load: true,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_session_load_failed")
    );
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(log.contains("\"method\":\"session/load\""), "log={log}");
    assert!(!log.contains("\"method\":\"session/new\""), "log={log}");
}

#[test]
fn acp_new_failure_returns_runtime_stage_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_new: true,
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
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_session_new_failed")
    );
}

#[test]
fn acp_missing_load_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        load_capability: false,
        configured_session_id: Some("sess-abc".to_string()),
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("validation_missing_acp_capability")
    );
    let details = result.details.expect("capability details");
    assert_eq!(details["required_capability"], "session/load");
    assert_eq!(details["target_session"], "bravo");
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/load\""), "log={log}");
}

#[test]
fn acp_missing_prompt_capability_returns_canonical_failure_code_and_details() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        prompt_capability: false,
        ..AcpStubOptions::default()
    };
    let (config_root, log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let (status, result) = chat_result(response);
    assert_eq!(status, ChatStatus::Failure);
    assert_eq!(result.outcome, ChatOutcome::Failed);
    assert_eq!(
        result.reason_code.as_deref(),
        Some("validation_missing_acp_capability")
    );
    let details = result.details.expect("capability details");
    assert_eq!(details["required_capability"], "session/prompt");
    assert_eq!(details["target_session"], "bravo");
    let log = fs::read_to_string(log_path).expect("read ACP log");
    assert!(!log.contains("\"method\":\"session/prompt\""), "log={log}");
}

#[test]
fn acp_initialize_failure_returns_canonical_runtime_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_initialize: true,
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
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_initialize_failed")
    );
    let details = result.details.expect("initialize details");
    assert_eq!(details["target_session"], "bravo");
}

#[test]
fn acp_prompt_failure_returns_runtime_stage_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        fail_prompt: true,
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
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_prompt_failed")
    );
}

#[test]
fn acp_disconnect_before_first_activity_returns_connection_closed_code() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("before_activity".to_string()),
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
    assert_eq!(
        result.reason_code.as_deref(),
        Some("runtime_acp_connection_closed")
    );
    assert_eq!(
        read_worker_state(temporary.path(), "bravo").as_deref(),
        Some("unavailable")
    );
}

#[test]
fn acp_disconnect_after_first_activity_preserves_accepted_response() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        disconnect_on_prompt: Some("after_activity".to_string()),
        update_count: 1,
        ..AcpStubOptions::default()
    };
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
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|value| value.get("delivery_phase")),
        Some(&Value::String("accepted_in_progress".to_string()))
    );
    assert_eq!(
        read_worker_state(temporary.path(), "bravo").as_deref(),
        Some("unavailable")
    );
}
