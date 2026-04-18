use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use agentmux::relay::{ChatDeliveryMode, ChatOutcome, ChatResult, ChatStatus, RelayResponse};
use agentmux::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn send_rejects_missing_message_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo"])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send without message");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_missing_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_conflicting_flag_and_piped_message_sources() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo", "--message", "hello"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_accepts_message_flag_when_piped_stdin_is_empty() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Chat {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            request_id: None,
            sender_session: "user".to_string(),
            sender_display_name: Some("Operator".to_string()),
            delivery_mode: ChatDeliveryMode::Async,
            status: ChatStatus::Accepted,
            results: vec![ChatResult {
                target_session: "bravo".to_string(),
                message_id: "msg-1".to_string(),
                outcome: ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        },
        Arc::clone(&request_log),
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--message",
            "hello from flag",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for child");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["message"], "hello from flag");
}

#[test]
fn send_rejects_conflicting_timeout_flags() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--quiescence-timeout-ms",
            "1000",
            "--acp-turn-timeout-ms",
            "2000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send with conflicting timeout flags");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_timeout_fields"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_invalid_acp_turn_timeout_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--message",
            "hello",
            "--acp-turn-timeout-ms",
            "0",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send with invalid ACP timeout");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_acp_turn_timeout"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_preserves_valid_explicit_session_in_relay_request() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Chat {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            request_id: None,
            sender_session: "user".to_string(),
            sender_display_name: Some("Alpha".to_string()),
            delivery_mode: ChatDeliveryMode::Async,
            status: ChatStatus::Accepted,
            results: vec![ChatResult {
                target_session: "bravo".to_string(),
                message_id: "msg-1".to_string(),
                outcome: ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        },
        Arc::clone(&request_log),
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--bundle",
            "agentmux",
            "--as-session",
            "user",
            "--target",
            "bravo",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin.write_all(b"hello").expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode send json payload");
    assert_eq!(payload["sender_session"], "user");

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "chat");
    assert_eq!(requests[0]["sender_session"], "user");
}

#[test]
fn send_rejects_unknown_explicit_session_without_fallback() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let workspace_root = temporary.path().join("workspace");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    fs::create_dir_all(&workspace_root).expect("create workspace root");
    write_bundle_configuration_with_member_directories(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &[
            ("alpha", workspace_root.as_path()),
            ("bravo", Path::new("/tmp")),
        ],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "send",
            "--bundle",
            "agentmux",
            "--as-session",
            "ghost",
            "--target",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send with unknown sender");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin.write_all(b"hello").expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_session"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("relay_unavailable"),
        "explicit session should fail before relay transport fallback: {stderr}"
    );
}

#[test]
fn send_rejects_sender_flag_in_mvp() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--sender",
            "relay",
            "--target",
            "bravo",
            "--message",
            "hello",
        ])
        .output()
        .expect("run send with sender flag");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument --sender"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_uses_tui_defaults_for_bundle_and_session() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(
        &config_root,
        "agentmux",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("agentmux"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Chat {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            request_id: None,
            sender_session: "user".to_string(),
            sender_display_name: Some("Operator".to_string()),
            delivery_mode: ChatDeliveryMode::Async,
            status: ChatStatus::Accepted,
            results: vec![ChatResult {
                target_session: "bravo".to_string(),
                message_id: "msg-1".to_string(),
                outcome: ChatOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            }],
        },
        Arc::clone(&request_log),
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "send",
            "--target",
            "bravo",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin.write_all(b"hello").expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    relay_thread.join().expect("join fake relay thread");
    assert!(output.status.success(), "command should succeed");

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "chat");
    assert_eq!(requests[0]["sender_session"], "user");
}
