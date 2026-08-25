//! `agentmux new peer` CLI surface: how a relay advisory reaches the operator.
//!
//! The relay is a separate process, so its stderr reaches nobody here. The
//! advisory arrives as payload and the CLI is what renders it.

use std::{
    collections::HashMap,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{RelayDiagnostic, RelayResponse};
use agentmux::runtime::paths::{
    BundleRuntimePaths, RelayRuntimePaths, ensure_bundle_runtime_directory,
};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

struct NewPeerFixture {
    _temporary: TempDir,
    config_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    inscriptions_root: std::path::PathBuf,
}

fn new_peer_fixture(
    diagnostics: Vec<RelayDiagnostic>,
) -> (NewPeerFixture, std::thread::JoinHandle<()>) {
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
            principal_id: "rnd-main@RELAY".to_string(),
            principal_type: "relay".to_string(),
            psk: Some("SECRET-PSK".to_string()),
            written_path: None,
            config_snippet: "# snippet".to_string(),
            diagnostics,
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
        request_logs,
    );
    (
        NewPeerFixture {
            _temporary: temporary,
            config_root,
            state_root,
            inscriptions_root,
        },
        relay_thread,
    )
}

fn run_new_peer(fixture: &NewPeerFixture, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["new", "peer", "rnd-main@RELAY", "--scope", "all"];
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

fn scope_advisory() -> Vec<RelayDiagnostic> {
    vec![RelayDiagnostic {
        code: "advisory_scope_resembles_policy_tier".to_string(),
        message: "ingress scope 'all' is a policy-tier value, not an ingress scope".to_string(),
    }]
}

#[test]
fn new_peer_renders_a_scope_advisory_to_stderr_and_still_succeeds() {
    let (fixture, relay_thread) = new_peer_fixture(scope_advisory());

    let output = run_new_peer(&fixture, &[]);
    relay_thread.join().expect("join fake relay");

    // The advisory reports a suspicion, not a fault: the principal was
    // registered, so the exit status must stay zero.
    assert!(
        output.status.success(),
        "an advisory must not fail the command"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("advisory_scope_resembles_policy_tier"),
        "the advisory must reach stderr: {stderr}"
    );
    assert!(
        stderr.contains("policy-tier value"),
        "the advisory message must be rendered, not only its code: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("psk=SECRET-PSK"),
        "the advisory must not disturb the credential output: {stdout}"
    );
}

#[test]
fn new_peer_json_mode_keeps_the_advisory_off_stdout() {
    let (fixture, relay_thread) = new_peer_fixture(scope_advisory());

    let output = run_new_peer(&fixture, &["--json"]);
    relay_thread.join().expect("join fake relay");
    assert!(
        output.status.success(),
        "an advisory must not fail the command"
    );

    // A caller parsing stdout as JSON must still get parseable JSON, so the
    // advisory belongs on stderr in this mode too.
    let payload: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must remain parseable JSON");
    assert_eq!(payload["principal_id"], "rnd-main@RELAY");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("advisory_scope_resembles_policy_tier"),
        "the advisory must reach stderr in json mode too: {stderr}"
    );
}

#[test]
fn new_peer_prints_nothing_to_stderr_when_no_advisory_was_raised() {
    let (fixture, relay_thread) = new_peer_fixture(Vec::new());

    let output = run_new_peer(&fixture, &[]);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "new peer should succeed");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("advisory_"),
        "no advisory may be printed when none was raised: {stderr}"
    );
}
