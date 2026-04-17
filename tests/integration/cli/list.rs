use std::{
    env, fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{
    ListedBundle, ListedBundleState, ListedSession, ListedSessionTransport, RelayError,
    RelayResponse,
};
use agentmux::runtime::association::WorkspaceContext;
use agentmux::runtime::paths::{BundleRuntimePaths, ensure_bundle_runtime_directory};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

fn discovered_sender_session_id() -> String {
    let current_directory = env::current_dir().expect("resolve current directory");
    let workspace = WorkspaceContext::discover(&current_directory).expect("discover workspace");
    workspace
        .auto_session_name()
        .expect("resolve sender session name")
}

#[test]
fn list_sessions_rejects_conflicting_bundle_and_all_selectors() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["list", "sessions", "--bundle", "alpha", "--all"])
        .output()
        .expect("run list sessions with conflicting selectors");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_params"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn list_sessions_requires_sessions_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["list"])
        .output()
        .expect("run list without subcommand");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_params"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn list_sessions_single_bundle_json_uses_canonical_bundle_shape() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    let sender_session = discovered_sender_session_id();
    let mut sessions = vec!["tui".to_string(), "master".to_string()];
    if !sessions.iter().any(|value| value == &sender_session) {
        sessions.push(sender_session);
    }
    let session_refs = sessions.iter().map(String::as_str).collect::<Vec<_>>();
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &session_refs);

    let bundle_paths = BundleRuntimePaths::resolve(&state_root, "agentmux").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &bundle_paths.relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "agentmux".to_string(),
                state: ListedBundleState::Up,
                state_reason_code: None,
                state_reason: None,
                sessions: vec![
                    ListedSession {
                        id: "tui".to_string(),
                        name: Some("GPT (Frontend Engineer)".to_string()),
                        transport: ListedSessionTransport::Tmux,
                    },
                    ListedSession {
                        id: "master".to_string(),
                        name: Some("GPT (Coordinator)".to_string()),
                        transport: ListedSessionTransport::Tmux,
                    },
                ],
            },
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "sessions",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions");
    relay_thread.join().expect("join fake relay thread");
    assert!(output.status.success(), "command should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode list payload");
    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle"]["id"], "agentmux");
    assert_eq!(payload["bundle"]["state"], "up");
    assert!(
        payload.get("recipients").is_none(),
        "legacy recipients payload must not be present: {payload}"
    );
    assert_eq!(
        payload["bundle"]["sessions"]
            .as_array()
            .expect("sessions array")
            .len(),
        2
    );

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "list");
}

#[test]
fn list_sessions_all_json_orders_bundles_lexicographically() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    let sender_session = discovered_sender_session_id();
    let mut sessions = vec!["tui".to_string()];
    if !sessions.iter().any(|value| value == &sender_session) {
        sessions.push(sender_session);
    }
    let session_refs = sessions.iter().map(String::as_str).collect::<Vec<_>>();
    write_bundle_configuration(&config_root, "beta", Some(&["dev"]), &session_refs);
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &session_refs);

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    let beta_paths = BundleRuntimePaths::resolve(&state_root, "beta").expect("beta paths");
    ensure_bundle_runtime_directory(&beta_paths).expect("ensure beta runtime directory");

    let alpha_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let beta_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let alpha_relay = spawn_fake_relay_once(
        &alpha_paths.relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "alpha".to_string(),
                state: ListedBundleState::Up,
                state_reason_code: None,
                state_reason: None,
                sessions: vec![ListedSession {
                    id: "tui".to_string(),
                    name: None,
                    transport: ListedSessionTransport::Tmux,
                }],
            },
        },
        Arc::clone(&alpha_log),
    );
    let beta_relay = spawn_fake_relay_once(
        &beta_paths.relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "beta".to_string(),
                state: ListedBundleState::Up,
                state_reason_code: None,
                state_reason: None,
                sessions: vec![ListedSession {
                    id: "tui".to_string(),
                    name: None,
                    transport: ListedSessionTransport::Tmux,
                }],
            },
        },
        Arc::clone(&beta_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "sessions",
            "--all",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions --all");
    alpha_relay.join().expect("join alpha relay");
    beta_relay.join().expect("join beta relay");
    assert!(output.status.success(), "command should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode list payload");
    let bundles = payload["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles.len(), 2);
    assert_eq!(bundles[0]["id"], "alpha");
    assert_eq!(bundles[1]["id"], "beta");
}

#[test]
fn list_sessions_all_fails_fast_on_first_authorization_denial() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    let sender_session = discovered_sender_session_id();
    let mut sessions = vec!["tui".to_string()];
    if !sessions.iter().any(|value| value == &sender_session) {
        sessions.push(sender_session);
    }
    let session_refs = sessions.iter().map(String::as_str).collect::<Vec<_>>();
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &session_refs);
    write_bundle_configuration(&config_root, "beta", Some(&["dev"]), &session_refs);

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    let alpha_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let alpha_relay = spawn_fake_relay_once(
        &alpha_paths.relay_socket,
        RelayResponse::Error {
            error: RelayError {
                code: "authorization_forbidden".to_string(),
                message: "list denied".to_string(),
                details: None,
            },
        },
        Arc::clone(&alpha_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "sessions",
            "--all",
            "--json",
            "--config-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions --all with denial");
    alpha_relay.join().expect("join alpha relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("authorization_forbidden"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("relay_unavailable"),
        "fanout must stop on first authorization denial: {stderr}"
    );
}
