//! `agentmux drop peer` CLI surface: the human and `--json` renderings of a
//! successful drop, and the non-zero exit a relay rejection produces.

use std::{
    collections::HashMap,
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use agentmux::relay::{RelayError, RelayResponse};
use agentmux::runtime::paths::{
    BundleRuntimePaths, RelayRuntimePaths, ensure_bundle_runtime_directory,
};
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

/// Lays out a single-bundle configuration whose CLI identity is the relay-wide
/// operator, and returns the roots plus the request log for the fake relay.
struct DropFixture {
    _temporary: TempDir,
    config_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    inscriptions_root: std::path::PathBuf,
    request_log: Arc<Mutex<Vec<Value>>>,
}

fn drop_fixture(response: RelayResponse) -> (DropFixture, std::thread::JoinHandle<()>) {
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

    let request_log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut responses = HashMap::new();
    responses.insert("alpha".to_string(), response);
    let mut request_logs = HashMap::new();
    request_logs.insert("alpha".to_string(), Arc::clone(&request_log));
    let relay_thread = spawn_fake_relay_for_bundles(
        &RelayRuntimePaths::resolve(&state_root).relay_socket,
        1,
        responses,
        request_logs,
    );
    (
        DropFixture {
            _temporary: temporary,
            config_root,
            state_root,
            inscriptions_root,
            request_log,
        },
        relay_thread,
    )
}

fn run_drop(fixture: &DropFixture, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["drop", "peer", "worker@alpha"];
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
        .expect("run agentmux drop peer")
}

fn session_drop_response() -> RelayResponse {
    RelayResponse::DropPeer {
        schema_version: "1".to_string(),
        principal_id: "worker@alpha".to_string(),
        principal_type: "session".to_string(),
        credential_path: Some("/state/sessions/worker/identity.psk".to_string()),
    }
}

#[test]
fn drop_peer_json_output_carries_the_payload_fields() {
    let (fixture, relay_thread) = drop_fixture(session_drop_response());

    let output = run_drop(&fixture, &["--json"]);
    relay_thread.join().expect("join fake relay");
    assert!(
        output.status.success(),
        "drop should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode drop payload");
    assert_eq!(payload["principal_id"], "worker@alpha");
    assert_eq!(payload["principal_type"], "session");
    assert_eq!(
        payload["credential_path"], "/state/sessions/worker/identity.psk",
        "the credential path must reach a machine caller: {payload}"
    );

    let requests = fixture.request_log.lock().expect("request log lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["operation"], "drop_peer",
        "unexpected relay request: {:?}",
        requests[0]
    );
    assert_eq!(requests[0]["principal_id"], "worker@alpha");
}

#[test]
fn drop_peer_human_output_names_the_credential_file_it_left_behind() {
    let (fixture, relay_thread) = drop_fixture(session_drop_response());

    let output = run_drop(&fixture, &[]);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "drop should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("principal_id=worker@alpha principal_type=session"),
        "unexpected drop output: {stdout}"
    );
    // The file is deliberately not deleted, so the human rendering has to say
    // where it is or the operator has no way to finish the cleanup.
    assert!(
        stdout.contains("credential file left in place at /state/sessions/worker/identity.psk"),
        "unexpected drop output: {stdout}"
    );
}

#[test]
fn drop_peer_json_omits_the_credential_path_for_a_peer_relay() {
    let (fixture, relay_thread) = drop_fixture(RelayResponse::DropPeer {
        schema_version: "1".to_string(),
        principal_id: "worker@alpha".to_string(),
        principal_type: "relay".to_string(),
        credential_path: None,
    });

    let output = run_drop(&fixture, &["--json"]);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "drop should succeed");

    let payload: Value = serde_json::from_slice(&output.stdout).expect("decode drop payload");
    // Absent, not null: the field is omitted for a principal the relay owns no
    // credential location for, matching the relay and MCP renderings.
    assert!(
        payload.get("credential_path").is_none(),
        "credential_path must be omitted rather than serialized as null: {payload}"
    );
    assert_eq!(payload["principal_type"], "relay");
}

#[test]
fn drop_peer_omits_the_credential_line_for_a_peer_relay() {
    let (fixture, relay_thread) = drop_fixture(RelayResponse::DropPeer {
        schema_version: "1".to_string(),
        principal_id: "worker@alpha".to_string(),
        principal_type: "relay".to_string(),
        credential_path: None,
    });

    let output = run_drop(&fixture, &[]);
    relay_thread.join().expect("join fake relay");
    assert!(output.status.success(), "drop should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("credential file left in place"),
        "no credential line may be printed for a path this relay cannot know: {stdout}"
    );
}

#[test]
fn drop_peer_exits_non_zero_when_the_relay_rejects_the_principal() {
    let (fixture, relay_thread) = drop_fixture(RelayResponse::Error {
        error: RelayError {
            code: "validation_unknown_principal".to_string(),
            message: "principal_id is not registered".to_string(),
            details: None,
        },
    });

    let output = run_drop(&fixture, &[]);
    relay_thread.join().expect("join fake relay");
    assert!(
        !output.status.success(),
        "an unregistered principal must not exit zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_unknown_principal"),
        "unexpected stderr: {stderr}"
    );
}
