//! `agentmux link peer` CLI coordination: the coordinator drives issuance on
//! one relay and installation on the other across two explicit state roots,
//! carrying the PSK in process memory only. Operation-routing fake relays
//! stand in for both sides; assertions inspect the recorded relay requests
//! and the operator-visible output, which must never carry secret material.

use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::helpers::*;
use agentmux::runtime::paths::RelayRuntimePaths;

pub(super) const ISSUER_SECRET: &str = "issuer-minted-psk-value";
pub(super) const ROTATED_SECRET: &str = "issuer-rotated-psk-value";
pub(super) const DISCARDED_MINT: &str = "discarded-registration-mint";
pub(super) const ISSUER_MINT: &str = "issuer-side-paired-mint";
pub(super) const DESTINATION_MINT: &str = "destination-side-paired-mint";

/// Fake relay routing on the request operation. Records every request on
/// `log` and answers through `responder`. Joined handles assert the
/// expected call count, so a coordinator that stops early (or over-calls)
/// fails the test.
///
/// A `None` response drops the connection without answering, simulating a
/// transport failure (the client observes EOF, not a relay decision).
pub(super) fn spawn_operation_fake(
    socket_path: &std::path::Path,
    expected_calls: usize,
    responder: impl Fn(Value) -> Option<Value> + Send + 'static,
    log: Arc<Mutex<Vec<Value>>>,
) -> JoinHandle<()> {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };
    if socket_path.exists() {
        fs::remove_file(socket_path).expect("remove stale relay socket");
    }
    let parent = socket_path.parent().expect("relay socket parent");
    fs::create_dir_all(parent).expect("create relay socket parent");
    let listener = UnixListener::bind(socket_path).expect("bind fake relay socket");
    listener
        .set_nonblocking(true)
        .expect("set fake relay listener nonblocking");
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let mut served = 0usize;
        let deadline = Instant::now() + Duration::from_secs(10);
        while served < expected_calls && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _address)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("set accepted stream blocking");
                    let mut reader =
                        BufReader::new(stream.try_clone().expect("clone fake relay stream"));
                    let mut hello_line = String::new();
                    reader
                        .read_line(&mut hello_line)
                        .expect("read fake relay hello");
                    let hello: Value = serde_json::from_str(hello_line.trim_end())
                        .expect("decode fake relay hello");
                    assert_eq!(hello.get("frame").and_then(Value::as_str), Some("hello"));
                    // Record the hello principal for per-side identity
                    // assertions; operation sequences filter these markers.
                    log.lock().expect("request log lock").push(json!({
                        "hello_principal_id": hello.get("principal_id").cloned().unwrap_or(Value::Null),
                    }));
                    let ack = json!({
                        "frame": "hello_ack",
                        "schema_version": "1",
                        "principal_id": hello.get("principal_id").cloned().unwrap_or(Value::Null),
                    });
                    let encoded_ack =
                        serde_json::to_string(&ack).expect("encode fake relay hello_ack");
                    stream
                        .write_all(encoded_ack.as_bytes())
                        .expect("write fake relay hello_ack");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay hello_ack");

                    let mut request_line = String::new();
                    reader
                        .read_line(&mut request_line)
                        .expect("read fake relay request");
                    let envelope: Value = serde_json::from_str(request_line.trim_end())
                        .expect("decode fake relay request envelope");
                    let request = envelope
                        .get("request")
                        .cloned()
                        .expect("fake relay request envelope missing 'request' field");
                    log.lock().expect("request log lock").push(request.clone());
                    let Some(payload) = responder(request) else {
                        // Transport failure: close without answering.
                        served += 1;
                        continue;
                    };
                    let response_frame = json!({
                        "frame": "response",
                        "request_id": envelope.get("request_id").cloned().unwrap_or(Value::Null),
                        "response": payload,
                    });
                    let encoded =
                        serde_json::to_string(&response_frame).expect("encode fake relay response");
                    stream
                        .write_all(encoded.as_bytes())
                        .expect("write fake relay response");
                    stream.write_all(b"\n").expect("write fake relay newline");
                    stream.flush().expect("flush fake relay response");
                    served += 1;
                }
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(source) => panic!("accept fake relay connection: {source}"),
            }
        }
        let _ = fs::remove_file(socket_path);
        assert_eq!(
            served, expected_calls,
            "fake relay did not serve all expected calls"
        );
    })
}

pub(super) fn new_peer_response(principal_id: &str, psk: Option<&str>) -> Value {
    json!({
        "kind": "new_peer",
        "schema_version": "1",
        "principal_id": principal_id,
        "principal_type": "relay",
        "psk": psk,
        "written_path": Value::Null,
        "config_snippet": "# snippet",
        "diagnostics": [],
    })
}

pub(super) fn install_response(alias: &str, written_path: &str) -> Value {
    json!({
        "kind": "install_peer_credential",
        "schema_version": "1",
        "alias": alias,
        "written_path": written_path,
    })
}

pub(super) fn relay_error_response(code: &str, message: &str) -> Value {
    json!({
        "kind": "error",
        "error": {"code": code, "message": message},
    })
}

pub(super) struct LinkSides {
    _temporary: TempDir,
    issuer_config: std::path::PathBuf,
    issuer_state: std::path::PathBuf,
    destination_config: std::path::PathBuf,
    destination_state: std::path::PathBuf,
    inscriptions: std::path::PathBuf,
}

pub(super) fn link_sides() -> LinkSides {
    let temporary = TempDir::new().expect("temporary directory");
    let paths = [
        "issuer-config",
        "issuer-state",
        "destination-config",
        "destination-state",
    ];
    for dir in paths {
        fs::create_dir_all(temporary.path().join(dir)).expect("create side directory");
    }
    let inscriptions = temporary.path().join("inscriptions");
    fs::create_dir_all(&inscriptions).expect("create inscriptions directory");
    let sides = LinkSides {
        issuer_config: temporary.path().join("issuer-config"),
        issuer_state: temporary.path().join("issuer-state"),
        destination_config: temporary.path().join("destination-config"),
        destination_state: temporary.path().join("destination-state"),
        inscriptions,
        _temporary: temporary,
    };
    for config in [&sides.issuer_config, &sides.destination_config] {
        write_bundle_configuration(config, "alpha", Some(&["dev"]), &["tui"]);
        write_tui_configuration(
            config,
            Some("alpha"),
            Some("user"),
            &[("user", "default", Some("Operator"))],
        );
    }
    sides
}

pub(super) fn issuer_socket(sides: &LinkSides) -> std::path::PathBuf {
    RelayRuntimePaths::resolve(&sides.issuer_state).relay_socket
}

pub(super) fn destination_socket(sides: &LinkSides) -> std::path::PathBuf {
    RelayRuntimePaths::resolve(&sides.destination_state).relay_socket
}

pub(super) fn run_link(sides: &LinkSides, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "link",
        "peer",
        "--issuer-state-directory",
        sides.issuer_state.to_str().expect("issuer state"),
        "--destination-state-directory",
        sides.destination_state.to_str().expect("destination state"),
        "--issuer-configuration-directory",
        sides.issuer_config.to_str().expect("issuer config"),
        "--destination-configuration-directory",
        sides
            .destination_config
            .to_str()
            .expect("destination config"),
        "--inscriptions-directory",
        sides.inscriptions.to_str().expect("inscriptions"),
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(args)
        .output()
        .expect("run agentmux link peer")
}

pub(super) fn operations(log: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
    log.lock()
        .expect("request log lock")
        .iter()
        .filter_map(|entry| {
            entry
                .get("operation")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn hello_principals(log: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
    log.lock()
        .expect("request log lock")
        .iter()
        .filter_map(|entry| {
            entry
                .get("hello_principal_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

pub(super) fn assert_no_secret(output: &std::process::Output, secrets: &[&str]) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for secret in secrets {
        assert!(
            !stdout.contains(secret),
            "stdout renders secret material: {stdout}"
        );
        assert!(
            !stderr.contains(secret),
            "stderr renders secret material: {stderr}"
        );
    }
}

// One-way link registers, issues, and installs across the two fakes
// without rendering either mint.
#[test]
fn link_peer_one_way_coordinates_without_rendering_psk() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |request| {
            assert_eq!(
                request.get("principal_id").and_then(Value::as_str),
                Some("bravo-link@RELAY")
            );
            Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET)))
        },
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT))),
            Some("install_peer_credential") => {
                assert_eq!(
                    request.get("psk").and_then(Value::as_str),
                    Some(ISSUER_SECRET),
                    "install must carry the issued PSK in memory: {request:?}"
                );
                Some(install_response(
                    "bravo",
                    "/fake/dest/state/peers/bravo.psk",
                ))
            }
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::clone(&destination_log),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(
        output.status.success(),
        "link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_secret(&output, &[ISSUER_SECRET, DISCARDED_MINT]);
    assert_eq!(
        operations(&issuer_log),
        vec!["new_peer".to_string()],
        "issuer must see issuance only"
    );
    assert_eq!(
        operations(&destination_log),
        vec![
            "new_peer".to_string(),
            "install_peer_credential".to_string()
        ],
        "destination must see registration then install"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Paired link cross-installs the two retained mints: the destination slot
// receives the issuer-side mint and vice versa.
#[test]
fn link_peer_paired_cross_installs_retained_mints() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("alpha@RELAY", Some(ISSUER_MINT))),
            Some("install_peer_credential") => {
                assert_eq!(
                    request.get("psk").and_then(Value::as_str),
                    Some(DESTINATION_MINT),
                    "issuer slot must receive the destination-side mint: {request:?}"
                );
                Some(install_response(
                    "alpha",
                    "/fake/issuer/state/peers/alpha.psk",
                ))
            }
            other => panic!("unexpected issuer operation: {other:?}"),
        },
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DESTINATION_MINT))),
            Some("install_peer_credential") => {
                assert_eq!(
                    request.get("psk").and_then(Value::as_str),
                    Some(ISSUER_MINT),
                    "destination slot must receive the issuer-side mint: {request:?}"
                );
                Some(install_response(
                    "bravo",
                    "/fake/dest/state/peers/bravo.psk",
                ))
            }
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::clone(&destination_log),
    );

    let output = run_link(
        &sides,
        &[
            "--alias",
            "bravo",
            "--connect-as",
            "alpha",
            "--paired",
            "--peer-alias",
            "alpha",
            "--peer-connect-as",
            "bravo",
        ],
    );
    assert!(
        output.status.success(),
        "paired link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_secret(&output, &[ISSUER_MINT, DESTINATION_MINT]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Declared cross-equality mismatch aborts before either relay is
// contacted.
#[test]
fn link_peer_paired_mismatch_contacts_no_relay() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        0,
        |_| panic!("issuer must not be contacted on mismatch"),
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        0,
        |_| panic!("destination must not be contacted on mismatch"),
        Arc::clone(&destination_log),
    );

    let output = run_link(
        &sides,
        &[
            "--alias",
            "bravo",
            "--connect-as",
            "wrong",
            "--paired",
            "--peer-alias",
            "alpha",
            "--peer-connect-as",
            "bravo",
        ],
    );
    assert!(!output.status.success(), "mismatch link must fail");
    assert!(
        operations(&issuer_log).is_empty() && operations(&destination_log).is_empty(),
        "mismatch must contact no relay"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Upgrade rotates the inbound identity instead of issuing, then installs
// the rotated credential — with no registration step on either side.
#[test]
fn link_peer_upgrade_rotates_then_installs() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |request| {
            assert_eq!(
                request.get("operation").and_then(Value::as_str),
                Some("change_psk"),
                "upgrade must rotate, not issue: {request:?}"
            );
            Some(json!({
                "kind": "change_psk",
                "schema_version": "1",
                "principal_id": "bravo-link@RELAY",
                "psk": ROTATED_SECRET,
                "written_path": Value::Null,
            }))
        },
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        1,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => {
                panic!("upgrade performs no registration: {request:?}")
            }
            Some("install_peer_credential") => {
                assert_eq!(
                    request.get("psk").and_then(Value::as_str),
                    Some(ROTATED_SECRET),
                    "install must carry the rotated PSK: {request:?}"
                );
                Some(install_response(
                    "bravo",
                    "/fake/dest/state/peers/bravo.psk",
                ))
            }
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::clone(&destination_log),
    );

    let output = run_link(
        &sides,
        &[
            "--alias",
            "bravo",
            "--connect-as",
            "bravo-link",
            "--upgrade",
        ],
    );
    assert!(
        output.status.success(),
        "upgrade link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_secret(&output, &[ROTATED_SECRET, DISCARDED_MINT]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Second-claim on alias registration proceeds to issuance and install.
#[test]
fn link_peer_tolerates_existing_alias_registration() {
    let sides = link_sides();
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |_| Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET))),
        Arc::new(Mutex::new(Vec::new())),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(relay_error_response(
                "validation_principal_exists",
                "principal_id is already registered",
            )),
            Some("install_peer_credential") => Some(install_response(
                "bravo",
                "/fake/dest/state/peers/bravo.psk",
            )),
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::clone(&destination_log),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(
        output.status.success(),
        "second-claim link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        operations(&destination_log),
        vec![
            "new_peer".to_string(),
            "install_peer_credential".to_string()
        ],
        "second-claim must proceed to install"
    );
    assert_no_secret(&output, &[ISSUER_SECRET]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Issuance failure halts before any install: the destination fake expects
// only the registration call.
#[test]
fn link_peer_halts_install_on_issuance_failure() {
    let sides = link_sides();
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |_| Some(relay_error_response("authorization_forbidden", "denied")),
        Arc::new(Mutex::new(Vec::new())),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        1,
        |request| {
            assert_eq!(
                request.get("operation").and_then(Value::as_str),
                Some("new_peer"),
                "only registration may precede a halted issuance: {request:?}"
            );
            Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT)))
        },
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(!output.status.success(), "failed issuance link must fail");
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// JSON output carries paths but never secret material.
#[test]
fn link_peer_json_output_omits_psk() {
    let sides = link_sides();
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |_| Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET))),
        Arc::new(Mutex::new(Vec::new())),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT))),
            Some("install_peer_credential") => Some(install_response(
                "bravo",
                "/fake/dest/state/peers/bravo.psk",
            )),
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(
        &sides,
        &["--alias", "bravo", "--connect-as", "bravo-link", "--json"],
    );
    assert!(
        output.status.success(),
        "link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(ISSUER_SECRET) && !stdout.contains(DISCARDED_MINT),
        "json output renders secret material: {stdout}"
    );
    let payload: Value = serde_json::from_str(&stdout).expect("parse link json output");
    assert_eq!(payload["alias"], "bravo");
    assert_eq!(payload["written_path"], "/fake/dest/state/peers/bravo.psk");
    assert!(
        payload.get("psk").is_none(),
        "json output must not carry a psk field: {payload}"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

// Unsafe peer tokens fail before either relay is contacted: no
// registration or issuance may precede the grammar check.
#[test]
fn link_peer_rejects_unsafe_tokens_without_contact() {
    for (flag, value) in [
        ("--alias", "../escape"),
        ("--alias", "a/b"),
        ("--connect-as", "with@qualifier"),
        ("--alias", "bang!alias"),
        ("--alias", "."),
    ] {
        let sides = link_sides();
        let issuer_log = Arc::new(Mutex::new(Vec::new()));
        let destination_log = Arc::new(Mutex::new(Vec::new()));
        let issuer = spawn_operation_fake(
            &issuer_socket(&sides),
            0,
            |_| panic!("issuer must not be contacted for an unsafe token"),
            Arc::clone(&issuer_log),
        );
        let destination = spawn_operation_fake(
            &destination_socket(&sides),
            0,
            |_| panic!("destination must not be contacted for an unsafe token"),
            Arc::clone(&destination_log),
        );
        let mut args = vec!["--alias", "bravo", "--connect-as", "bravo-link"];
        let position = args
            .iter()
            .position(|item| *item == flag)
            .expect("flag present");
        args[position + 1] = value;
        let output = run_link(&sides, &args);
        assert!(
            !output.status.success(),
            "unsafe token {flag}={value} must fail"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("is not a safe peer token"),
            "must report the token grammar: {stderr}"
        );
        assert!(
            operations(&issuer_log).is_empty() && operations(&destination_log).is_empty(),
            "unsafe token must contact no relay"
        );
        issuer.join().expect("join issuer fake");
        destination.join().expect("join destination fake");
    }
}

// Relays with differing operator identities link through per-side
// selectors: each fake observes its own side's hello principal.
#[test]
fn link_peer_uses_per_side_operator_identities() {
    let temporary = TempDir::new().expect("temporary directory");
    let paths = [
        "issuer-config",
        "issuer-state",
        "destination-config",
        "destination-state",
    ];
    for dir in paths {
        fs::create_dir_all(temporary.path().join(dir)).expect("create side directory");
    }
    let inscriptions = temporary.path().join("inscriptions");
    fs::create_dir_all(&inscriptions).expect("create inscriptions directory");
    let issuer_config = temporary.path().join("issuer-config");
    let issuer_state = temporary.path().join("issuer-state");
    let destination_config = temporary.path().join("destination-config");
    let destination_state = temporary.path().join("destination-state");
    for config in [&issuer_config, &destination_config] {
        write_bundle_configuration(config, "alpha", Some(&["dev"]), &["tui"]);
    }
    write_tui_configuration(
        &issuer_config,
        Some("alpha"),
        Some("op-issuer"),
        &[("op-issuer", "default", None)],
    );
    write_tui_configuration(
        &destination_config,
        Some("alpha"),
        Some("op-dest"),
        &[("op-dest", "default", None)],
    );

    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &RelayRuntimePaths::resolve(&issuer_state).relay_socket,
        1,
        |_| Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET))),
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &RelayRuntimePaths::resolve(&destination_state).relay_socket,
        2,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT))),
            Some("install_peer_credential") => Some(install_response(
                "bravo",
                "/fake/dest/state/peers/bravo.psk",
            )),
            other => panic!("unexpected destination operation: {other:?}"),
        },
        Arc::clone(&destination_log),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "link",
            "peer",
            "--issuer-state-directory",
            issuer_state.to_str().expect("issuer state"),
            "--destination-state-directory",
            destination_state.to_str().expect("destination state"),
            "--issuer-configuration-directory",
            issuer_config.to_str().expect("issuer config"),
            "--destination-configuration-directory",
            destination_config.to_str().expect("destination config"),
            "--inscriptions-directory",
            inscriptions.to_str().expect("inscriptions"),
            "--alias",
            "bravo",
            "--connect-as",
            "bravo-link",
            "--issuer-as-session",
            "op-issuer@GLOBAL",
            "--destination-as-session",
            "op-dest@GLOBAL",
        ])
        .output()
        .expect("run agentmux link peer");
    assert!(
        output.status.success(),
        "per-side link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let issuer_hellos = hello_principals(&issuer_log);
    let destination_hellos = hello_principals(&destination_log);
    assert_eq!(issuer_hellos.len(), 1, "issuer must see one hello");
    assert_eq!(
        destination_hellos.len(),
        2,
        "destination must see two hellos"
    );
    assert!(
        issuer_hellos[0].contains("op-issuer"),
        "issuer must authenticate as its own operator: {issuer_hellos:?}"
    );
    assert!(
        destination_hellos
            .iter()
            .all(|principal| principal.contains("op-dest")),
        "destination must authenticate as its own operator: {destination_hellos:?}"
    );
    assert_ne!(
        issuer_hellos[0], destination_hellos[0],
        "per-side selectors must reach independent endpoints"
    );
    assert_no_secret(&output, &[ISSUER_SECRET, DISCARDED_MINT]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

#[test]
fn link_help_output_includes_usage_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["link", "--help"])
        .output()
        .expect("run agentmux link --help");
    assert!(output.status.success(), "link help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux link peer"),
        "unexpected link help output: {stdout}"
    );
}
