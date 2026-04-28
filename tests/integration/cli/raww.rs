use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{ListedSessionTransport, RelayError, RelayResponse};
use agentmux::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn raww_rejects_missing_text_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["raww", "bravo"])
        .output()
        .expect("run raww without text");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_params"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn raww_forwards_no_enter_and_preserves_json_contract() {
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
        RelayResponse::Raww {
            schema_version: "1".to_string(),
            status: "accepted".to_string(),
            target_session: "bravo".to_string(),
            transport: ListedSessionTransport::Acp,
            request_id: None,
            message_id: Some("raww-msg-1".to_string()),
            details: Some(serde_json::json!({
                "delivery_phase": "accepted_in_progress",
            })),
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "raww",
            "bravo",
            "--text",
            "echo test",
            "--no-enter",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run raww");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode raww json payload");
    assert_eq!(payload["status"], "accepted");
    assert_eq!(payload["target_session"], "bravo");
    assert_eq!(payload["transport"], "acp");
    assert_eq!(payload["message_id"], "raww-msg-1");
    assert_eq!(payload["details"]["delivery_phase"], "accepted_in_progress");

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "raww");
    assert_eq!(requests[0]["sender_session"], "user");
    assert_eq!(requests[0]["target_session"], "bravo");
    assert_eq!(requests[0]["text"], "echo test");
    assert_eq!(requests[0]["no_enter"], true);
}

#[test]
fn raww_surfaces_unknown_target_from_relay() {
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
    let workspace_root = temporary.path().join("workspace");
    fs::create_dir_all(&workspace_root).expect("create workspace");

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "validation_unknown_target".to_string(),
                message: "target_session is not a canonical configured target identifier"
                    .to_string(),
                details: Some(serde_json::json!({
                    "target_session": "ghost",
                })),
            },
        },
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "raww",
            "ghost",
            "--text",
            "echo test",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run raww with unknown target");
    relay_thread.join().expect("join fake relay thread");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_target"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn raww_rejects_unknown_explicit_session_without_association_fallback() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let workspace_root = temporary.path().join("workspace");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    fs::create_dir_all(&workspace_root).expect("create workspace root");
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

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "raww",
            "bravo",
            "--text",
            "echo test",
            "--bundle",
            "agentmux",
            "--as-session",
            "ghost",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run raww with unknown as-session");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_session"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("relay_unavailable"),
        "unknown session should fail before relay transport fallback: {stderr}"
    );
}
