use std::{
    collections::HashMap,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{
    ListedBundle, ListedBundleStartupHealth, ListedBundleState, ListedSession,
    ListedSessionTransport, RelayError, RelayResponse, StartupFailureRecord,
};
use agentmux::runtime::paths::{
    BundleRuntimePaths, RelayRuntimePaths, ensure_bundle_runtime_directory,
};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn list_principals_rejects_reserved_namespace_token() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["list", "principals", "--namespace", "EXTERNAL"])
        .output()
        .expect("run list principals with reserved namespace");
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
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "master"]);
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
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "agentmux".to_string(),
                hosted: true,
                state: ListedBundleState::Up,
                startup_health: Some(ListedBundleStartupHealth::Healthy),
                state_reason_code: None,
                state_reason: None,
                startup_failure_count: 0,
                recent_startup_failures: Vec::new(),
                principals: vec![
                    ListedSession {
                        id: "tui".to_string(),
                        name: Some("GPT (Frontend Engineer)".to_string()),
                        transport: ListedSessionTransport::Tmux,
                        ready: true,
                    },
                    ListedSession {
                        id: "master".to_string(),
                        name: Some("GPT (Coordinator)".to_string()),
                        transport: ListedSessionTransport::Tmux,
                        ready: true,
                    },
                ],
                principals_partial: None,
            },
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--json",
            "--configuration-directory",
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
    assert_eq!(payload["bundle"]["hosted"], true);
    assert_eq!(payload["bundle"]["startup_health"], "healthy");
    assert_eq!(payload["bundle"]["startup_failure_count"], 0);
    assert!(
        payload["bundle"]["recent_startup_failures"]
            .as_array()
            .is_some_and(|values| values.is_empty())
    );
    assert!(
        payload.get("recipients").is_none(),
        "legacy recipients payload must not be present: {payload}"
    );
    assert_eq!(
        payload["bundle"]["principals"]
            .as_array()
            .expect("sessions array")
            .len(),
        2
    );

    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["operation"], "list");
    assert_eq!(requests[0]["requester_session"], "user@GLOBAL");
}

#[test]
fn list_sessions_human_output_renders_per_session_startup_failures() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "agentmux", Some(&["dev"]), &["tui", "master"]);
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
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "agentmux".to_string(),
                hosted: true,
                state: ListedBundleState::Down,
                startup_health: None,
                state_reason_code: Some("startup_failed".to_string()),
                state_reason: None,
                startup_failure_count: 1,
                recent_startup_failures: vec![StartupFailureRecord {
                    session_id: "tui".to_string(),
                    transport: ListedSessionTransport::Tmux,
                    code: "runtime_startup_failed".to_string(),
                    reason: "opencode: command not found".to_string(),
                    timestamp: "2026-07-02T00:00:00Z".to_string(),
                    sequence: 1,
                    details: None,
                }],
                principals: vec![ListedSession {
                    id: "tui".to_string(),
                    name: None,
                    transport: ListedSessionTransport::Tmux,
                    ready: false,
                }],
                principals_partial: None,
            },
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--configuration-directory",
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

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("startup_failure_count=1"),
        "header should carry the aggregate count: {stdout}"
    );
    assert!(
        stdout.contains(
            "  startup_failure session=tui code=runtime_startup_failed \
             reason=opencode: command not found"
        ),
        "human output should render the per-session failure reason: {stdout}"
    );
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
    write_bundle_configuration(&config_root, "beta", Some(&["dev"]), &["tui"]);
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["tui"]);
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    let beta_paths = BundleRuntimePaths::resolve(&state_root, "beta").expect("beta paths");
    ensure_bundle_runtime_directory(&beta_paths).expect("ensure beta runtime directory");

    let alpha_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let beta_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut responses = HashMap::new();
    responses.insert(
        "alpha".to_string(),
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "alpha".to_string(),
                hosted: true,
                state: ListedBundleState::Up,
                startup_health: Some(ListedBundleStartupHealth::Healthy),
                state_reason_code: None,
                state_reason: None,
                startup_failure_count: 0,
                recent_startup_failures: Vec::new(),
                principals: vec![ListedSession {
                    id: "tui".to_string(),
                    name: None,
                    transport: ListedSessionTransport::Tmux,
                    ready: true,
                }],
                principals_partial: None,
            },
        },
    );
    responses.insert(
        "beta".to_string(),
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "beta".to_string(),
                hosted: true,
                state: ListedBundleState::Up,
                startup_health: Some(ListedBundleStartupHealth::Healthy),
                state_reason_code: None,
                state_reason: None,
                startup_failure_count: 0,
                recent_startup_failures: Vec::new(),
                principals: vec![ListedSession {
                    id: "tui".to_string(),
                    name: None,
                    transport: ListedSessionTransport::Tmux,
                    ready: true,
                }],
                principals_partial: None,
            },
        },
    );
    let mut request_logs = HashMap::new();
    request_logs.insert("alpha".to_string(), Arc::clone(&alpha_log));
    request_logs.insert("beta".to_string(), Arc::clone(&beta_log));
    let relay_thread = spawn_fake_relay_for_bundles(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        2,
        responses,
        request_logs,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "list",
            "principals",
            "--namespace",
            "*",
            "--json",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions --all");
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "command should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode list payload");
    let bundles = payload["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles.len(), 2);
    assert_eq!(bundles[0]["id"], "alpha");
    assert_eq!(bundles[1]["id"], "beta");
    assert_eq!(bundles[0]["startup_health"], "healthy");
    assert_eq!(bundles[0]["startup_failure_count"], 0);
    assert_eq!(bundles[1]["startup_health"], "healthy");
    assert_eq!(bundles[1]["startup_failure_count"], 0);
    let alpha_requests = alpha_log.lock().expect("alpha request lock");
    assert_eq!(alpha_requests[0]["requester_session"], "user@GLOBAL");
    let beta_requests = beta_log.lock().expect("beta request lock");
    assert_eq!(beta_requests[0]["requester_session"], "user@GLOBAL");
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
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["tui"]);
    write_bundle_configuration(&config_root, "beta", Some(&["dev"]), &["tui"]);
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");
    let alpha_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let alpha_relay = spawn_fake_relay_once(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
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
            "principals",
            "--namespace",
            "*",
            "--json",
            "--configuration-directory",
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

#[test]
fn list_sessions_uses_ui_session_identity_defaults_outside_associated_workspace() {
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
        "infrastructure",
        Some(&["dev"]),
        &["alpha", "bravo"],
    );
    write_tui_configuration(
        &config_root,
        Some("infrastructure"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );

    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, "infrastructure").expect("bundle paths");
    ensure_bundle_runtime_directory(&bundle_paths).expect("ensure bundle runtime directory");
    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let relay_thread = spawn_fake_relay_once(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        RelayResponse::List {
            schema_version: "1".to_string(),
            bundle: ListedBundle {
                id: "infrastructure".to_string(),
                hosted: true,
                state: ListedBundleState::Up,
                startup_health: Some(ListedBundleStartupHealth::Healthy),
                state_reason_code: None,
                state_reason: None,
                startup_failure_count: 0,
                recent_startup_failures: Vec::new(),
                principals: vec![],
                principals_partial: None,
            },
        },
        Arc::clone(&request_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(&workspace_root)
        .args([
            "list",
            "principals",
            "--namespace",
            "infrastructure",
            "--json",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run list sessions --bundle infrastructure");
    relay_thread.join().expect("join fake relay thread");

    assert!(output.status.success(), "command should succeed");
    let requests = request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["requester_session"], "user@GLOBAL");
}
