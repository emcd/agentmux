use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{RelayError, RelayResponse};
use agentmux::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn look_returns_canonical_json_payload() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Look {
            schema_version: "1".to_string(),
            bundle_name: "agentmux".to_string(),
            requester_session: "tui".to_string(),
            target_session: "bravo".to_string(),
            captured_at: "2026-03-08T00:00:00Z".to_string(),
            snapshot_lines: vec!["LOOK-A".to_string(), "LOOK-B".to_string()],
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode look json payload");
    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle_name"], "agentmux");
    assert_eq!(payload["requester_session"], "tui");
    assert_eq!(payload["target_session"], "bravo");
    assert_eq!(payload["captured_at"], "2026-03-08T00:00:00Z");
    assert_eq!(
        payload["snapshot_lines"],
        serde_json::json!(["LOOK-A", "LOOK-B"])
    );

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "look");
    assert_eq!(requests[0]["requester_session"], "tui");
    assert_eq!(requests[0]["target_session"], "bravo");
}

#[test]
fn look_rejects_cross_bundle_request_in_mvp() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--bundle",
            "other-bundle",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look --bundle");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_cross_bundle_unsupported"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_surfaces_authorization_forbidden_from_relay() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "authorization_forbidden".to_string(),
                message: "request denied by authorization policy".to_string(),
                details: Some(serde_json::json!({
                    "capability": "look.inspect",
                    "requester_session": "tui",
                    "bundle_name": "agentmux",
                    "target_session": "bravo",
                    "reason": "look policy scope permits self-only inspection",
                })),
            },
        },
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("authorization_forbidden"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_surfaces_unsupported_transport_from_relay() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "bravo"]);
    let workspace_root = temporary.path().join("workspace");
    configure_local_mcp_override(&workspace_root, "agentmux", "tui");

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "validation_unsupported_transport".to_string(),
                message: "look is unsupported for ACP targets in MVP".to_string(),
                details: Some(serde_json::json!({
                    "target_session": "bravo",
                    "transport": "acp",
                })),
            },
        },
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "look",
            "bravo",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux look");
    relay_thread.join().expect("join fake relay thread");

    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unsupported_transport"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn look_rejects_invalid_lines_bounds() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["look", "bravo", "--lines", "0"])
        .output()
        .expect("run agentmux look --lines 0");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_lines"),
        "unexpected stderr: {stderr}"
    );
}
