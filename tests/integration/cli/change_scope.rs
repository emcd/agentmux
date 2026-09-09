//! `agentmux change scope` CLI surface: the canonical scope reaches stdout,
//! relay advisories reach stderr, and a missing `--scope` fails before any
//! relay contact.

use std::{
    collections::HashMap,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::RelayResponse;
use agentmux::runtime::paths::{
    BundleRuntimePaths, RelayRuntimePaths, ensure_bundle_runtime_directory,
};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

struct ChangeScopeFixture {
    _temporary: TempDir,
    config_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    inscriptions_root: std::path::PathBuf,
    request_logs: HashMap<String, Arc<Mutex<Vec<Value>>>>,
}

fn change_scope_fixture(
    expected_calls: usize,
) -> (ChangeScopeFixture, std::thread::JoinHandle<()>) {
    let temporary = TempDir::new().expect("temporary directory");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["tui"]);
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );
    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");

    let mut responses = HashMap::new();
    responses.insert(
        "alpha".to_string(),
        RelayResponse::ChangeScope {
            schema_version: "1".to_string(),
            principal_id: "west@RELAY".to_string(),
            scope: "alpha,beta".to_string(),
            diagnostics: Vec::new(),
        },
    );
    let mut request_logs = HashMap::new();
    request_logs.insert(
        "alpha".to_string(),
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );
    let relay_thread = spawn_fake_relay_for_bundles(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        expected_calls,
        responses,
        request_logs.clone(),
    );
    (
        ChangeScopeFixture {
            _temporary: temporary,
            config_root,
            state_root,
            inscriptions_root,
            request_logs,
        },
        relay_thread,
    )
}

fn run_change_scope(extra: &[&str], fixture: &ChangeScopeFixture) -> std::process::Output {
    let mut args = vec!["change", "scope", "west@RELAY"];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(args)
        .args([
            "--configuration-directory",
            &fixture.config_root.to_string_lossy(),
            "--state-directory",
            &fixture.state_root.to_string_lossy(),
            "--inscriptions-directory",
            &fixture.inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux change scope")
}

#[test]
fn change_scope_renders_the_canonical_scope_and_forwards_it() {
    let (fixture, relay_thread) = change_scope_fixture(1);

    let output = run_change_scope(&["--scope", "beta,alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "change scope should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scope=alpha,beta"),
        "stdout must carry the canonical scope: {stdout}"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert_eq!(logged.len(), 1, "one relay request: {logged:?}");
    assert_eq!(logged[0]["operation"], "change_scope");
    assert_eq!(logged[0]["principal_id"], "west@RELAY");
    assert_eq!(logged[0]["scope"], "beta,alpha");
}

#[test]
fn change_scope_json_mode_reports_the_canonical_scope() {
    let (fixture, relay_thread) = change_scope_fixture(1);

    let output = run_change_scope(&["--scope", "beta,alpha", "--json"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "change scope should succeed");

    let payload: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must remain parseable JSON");
    assert_eq!(payload["principal_id"], "west@RELAY");
    assert_eq!(payload["scope"], "alpha,beta");
}

#[test]
fn change_scope_requires_an_explicit_scope_argument() {
    let (fixture, relay_thread) = change_scope_fixture(0);

    let output = run_change_scope(&[], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "a missing --scope must fail before relay contact"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert!(
        logged.is_empty(),
        "no relay request may be issued: {logged:?}"
    );
}

fn run_change_scope_for(
    principal: &str,
    extra: &[&str],
    fixture: &ChangeScopeFixture,
) -> std::process::Output {
    let mut args = vec!["change", "scope", principal];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(args)
        .args([
            "--configuration-directory",
            &fixture.config_root.to_string_lossy(),
            "--state-directory",
            &fixture.state_root.to_string_lossy(),
            "--inscriptions-directory",
            &fixture.inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux change scope")
}

#[test]
fn change_scope_rejects_malformed_scope_before_relay_contact() {
    let (fixture, relay_thread) = change_scope_fixture(0);

    let output = run_change_scope_for("west@RELAY", &["--scope", "*,alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "a malformed scope must fail before relay contact"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert!(
        logged.is_empty(),
        "no relay request may be issued: {logged:?}"
    );
}

#[test]
fn change_scope_rejects_non_relay_principal_before_relay_contact() {
    let (fixture, relay_thread) = change_scope_fixture(0);

    let output = run_change_scope_for("worker@party", &["--scope", "alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "a non-relay principal must fail before relay contact"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert!(
        logged.is_empty(),
        "no relay request may be issued: {logged:?}"
    );
}

fn uncertain_fixture(expected_calls: usize) -> (ChangeScopeFixture, std::thread::JoinHandle<()>) {
    let temporary = TempDir::new().expect("temporary directory");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["tui"]);
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );
    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");

    let mut responses = HashMap::new();
    responses.insert(
        "alpha".to_string(),
        RelayResponse::Error {
            error: agentmux::relay::RelayError {
                code: "internal_store_durability_uncertain".to_string(),
                message: "scope replacement published but durability is uncertain".to_string(),
                details: Some(serde_json::json!({
                    "principal_id": "west@RELAY",
                    "scope": "alpha",
                })),
            },
        },
    );
    let relay_thread = spawn_fake_relay_for_bundles(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        expected_calls,
        responses,
        HashMap::new(),
    );
    (
        ChangeScopeFixture {
            _temporary: temporary,
            config_root,
            state_root,
            inscriptions_root,
            request_logs: HashMap::new(),
        },
        relay_thread,
    )
}

#[test]
fn change_scope_reports_uncertain_durability_with_effective_scope() {
    let (fixture, relay_thread) = uncertain_fixture(1);

    let output = run_change_scope(&["--scope", "alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "uncertainty must exit nonzero: {output:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scope=alpha") && stdout.contains("durability=uncertain"),
        "stdout must preserve the effective grant: {stdout}"
    );
}

#[test]
fn change_scope_json_reports_uncertain_durability_with_effective_scope() {
    let (fixture, relay_thread) = uncertain_fixture(1);

    let output = run_change_scope(&["--scope", "alpha", "--json"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "uncertainty must exit nonzero: {output:?}"
    );
    let payload: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must remain parseable JSON");
    assert_eq!(payload["scope"], "alpha");
    assert_eq!(payload["durability"], "uncertain");
    assert_eq!(payload["principal_id"], "west@RELAY");
}

#[test]
fn change_scope_rejects_a_bare_namespace_identity_before_relay_contact() {
    let (fixture, relay_thread) = change_scope_fixture(0);

    let output = run_change_scope_for("@RELAY", &["--scope", "alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "a namespace without a local identity must fail before relay contact"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert!(
        logged.is_empty(),
        "no relay request may be issued: {logged:?}"
    );
}

#[test]
fn change_scope_normalizes_a_padded_principal_before_relay_contact() {
    let (fixture, relay_thread) = change_scope_fixture(1);

    let output = run_change_scope_for(" west@RELAY", &["--scope", "alpha"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "padded principal: {output:?}");
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert_eq!(logged.len(), 1);
    assert_eq!(
        logged[0]["principal_id"], "west@RELAY",
        "the normalized identity is submitted: {logged:?}"
    );
}

fn run_new_peer(
    principal: &str,
    extra: &[&str],
    fixture: &ChangeScopeFixture,
) -> std::process::Output {
    let mut args = vec!["new", "peer", principal];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(args)
        .args([
            "--configuration-directory",
            &fixture.config_root.to_string_lossy(),
            "--state-directory",
            &fixture.state_root.to_string_lossy(),
            "--inscriptions-directory",
            &fixture.inscriptions_root.to_string_lossy(),
        ])
        .output()
        .expect("run agentmux new peer")
}

#[test]
fn new_peer_rejects_a_bare_namespace_identity_before_relay_contact() {
    let (fixture, relay_thread) = change_scope_fixture(0);

    let output = run_new_peer("@RELAY", &["--scope", "*"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "a namespace without a local identity must fail before relay contact"
    );
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert!(
        logged.is_empty(),
        "no relay request may be issued: {logged:?}"
    );
}

#[test]
fn new_peer_normalizes_a_padded_principal_before_relay_contact() {
    let temporary = TempDir::new().expect("temporary directory");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration(&config_root, "alpha", Some(&["dev"]), &["tui"]);
    write_tui_configuration(
        &config_root,
        Some("alpha"),
        Some("user"),
        &[("user", "default", Some("Operator"))],
    );
    let alpha_paths = BundleRuntimePaths::resolve(&state_root, "alpha").expect("alpha paths");
    ensure_bundle_runtime_directory(&alpha_paths).expect("ensure alpha runtime directory");

    let mut responses = HashMap::new();
    responses.insert(
        "alpha".to_string(),
        RelayResponse::NewPeer {
            schema_version: "1".to_string(),
            principal_id: "west@RELAY".to_string(),
            principal_type: "relay".to_string(),
            psk: Some("SECRET-PSK".to_string()),
            written_path: None,
            config_snippet: "# snippet".to_string(),
            diagnostics: Vec::new(),
        },
    );
    let mut request_logs = HashMap::new();
    request_logs.insert(
        "alpha".to_string(),
        Arc::new(Mutex::new(Vec::<Value>::new())),
    );
    let relay_thread = spawn_fake_relay_for_bundles(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        1,
        responses,
        request_logs.clone(),
    );
    let fixture = ChangeScopeFixture {
        _temporary: temporary,
        config_root,
        state_root,
        inscriptions_root,
        request_logs,
    };

    let output = run_new_peer(" west@RELAY", &["--scope", "*"], &fixture);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "padded principal: {output:?}");
    let logged = fixture.request_logs["alpha"]
        .lock()
        .expect("lock request log");
    assert_eq!(logged.len(), 1);
    assert_eq!(
        logged[0]["principal_id"], "west@RELAY",
        "the normalized identity is submitted: {logged:?}"
    );
}
