//! `change scope` peer-grant administration and live multi-namespace ingress:
//! - [`grammar`]: namespace-set/wildcard grammar, canonical persistence, and
//!   malformed-scope rejection.
//! - [`updates`]: in-place credential-preserving updates (update, clear,
//!   no-op), the dedicated `change.scope` authority, live narrowing/widening
//!   on one connection for Send/Raww plus both discovery operations, rotation
//!   preserving the narrowed scope, and wildcard reach.
//! - [`binding`]: drop/revocation and rotation isolation of stale bindings.
//! - [`faults`]: pre-commit failure preservation, post-rename
//!   durability-uncertainty effectiveness and retry, and rotation recovery.
//! - [`concurrency`]: shared-context serialization of concurrent updates and
//!   rotation, admitted-work survival, and admin progress during a stalled
//!   principal probe.
//!
//! Shared helpers (the scope-grant operator configuration, the admin request
//! dispatcher, store inspection, and the peer send prober) live in this hub.
//! Cluster-specific helpers live with their cluster.
//!

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{
    Arc, Barrier, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use agentmux::configuration::ConfigurationRoots;
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Writes the identity-test operator configuration plus an explicit
/// `change.scope=all` grant, so scope updates authorize.
fn write_scope_configuration(temporary: &TempDir, bundle_name: &str) -> ConfigurationRoots {
    let configuration_roots = write_bundle_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"
scope = "all"

[policies.controls.drop]
peer = "all"
"#,
    )
    .expect("write operator policies configuration");
    write_tui_configuration(&configuration_roots, "operator", bundle_name);
    configuration_roots
}

/// Submits one `change_scope` admin request as the `@GLOBAL` operator.
fn change_scope_request(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    scope: Option<&str>,
) -> Value {
    let mut request = json!({"operation": "change_scope", "principal_id": principal_id});
    if let Some(scope) = scope {
        request["scope"] = Value::String(scope.to_string());
    }
    operator_request(configuration_roots, bundle_paths, bundle_name, request)
}

/// Reads one principal record from the on-disk store.
fn store_record(bundle_paths: &BundleRuntimePaths, principal_id: &str) -> Value {
    let raw =
        std::fs::read_to_string(principal_store_file(bundle_paths)).expect("read principal store");
    let store: Value = serde_json::from_str(&raw).expect("parse principal store");
    store["principals"]
        .as_array()
        .expect("principals array")
        .iter()
        .find(|record| record["principal_id"] == principal_id)
        .unwrap_or_else(|| panic!("missing record for {principal_id}: {store}"))
        .clone()
}

/// Writes a users.toml declaring `operators` distinct `@GLOBAL` operators on
/// the operator policy, so concurrent admin callers on one shared context do
/// not collide on a single identity claim.
fn write_multi_operator_users(configuration_roots: &ConfigurationRoots, operators: &[String]) {
    let sessions = operators
        .iter()
        .map(|operator| {
            format!("[[sessions]]\nid = \"{operator}\"\npolicy = \"operator\"\n\n[sessions.ui]\n")
        })
        .collect::<String>();
    std::fs::write(
        configuration_roots.base_layer().join("users.toml"),
        format!("default-session = \"{}\"\n\n{}", operators[0], sessions),
    )
    .expect("write multi-operator users configuration");
}

/// Connects as an explicit `@GLOBAL` operator, submits one relay-admin
/// request, and returns the response frame. Variant of the shared
/// `operator_request` for tests that hold another operator identity live.
fn operator_request_as(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    operator: &str,
    request: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "operator hello not acked: {ack:?}"
    );
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "admin-1",
            "request": request,
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    response
}

/// Registers `principal_id` via `new peer` as an explicit operator and
/// returns the issued PSK.
fn register_peer_as(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    operator: &str,
    principal_id: &str,
    scope: Option<&str>,
) -> String {
    let mut request = json!({"operation": "new_peer", "principal_id": principal_id});
    if let Some(scope) = scope {
        request["scope"] = Value::String(scope.to_string());
    }
    let response = operator_request_as(configuration_roots, bundle_paths, operator, request);
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer rejected: {response:?}"
    );
    response["response"]["psk"]
        .as_str()
        .expect("psk in new peer response")
        .to_string()
}
/// Submits one ingress Send as a peer relay over a fresh connection.
fn peer_send_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    peer_id: &str,
    psk: &str,
    target: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": peer_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "peer-send",
            "request": {
                "operation": "send",
                "requester_session": peer_id,
                "message": "scope probe",
                "targets": [target],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer relay thread");
    response
}

/// Black-holes a bundle's tmux socket: the tmux CLI dials it during readiness
/// probes and delivery attempts, then waits indefinitely for a reply that
/// never comes, stalling the calling worker until released. Lets tests hold
/// target-side execution at a deterministic point.
struct TmuxBlackhole {
    stop: Arc<AtomicBool>,
    accepted: Arc<Mutex<Vec<UnixStream>>>,
    thread: Option<thread::JoinHandle<()>>,
    dialed_rx: mpsc::Receiver<()>,
}

impl TmuxBlackhole {
    fn arm(runtime_dir: &Path) -> Self {
        std::fs::create_dir_all(runtime_dir).expect("runtime dir");
        let socket_path = runtime_dir.join("tmux.sock");
        // tmux sockets must fit the platform sun_path (104 bytes on macOS):
        // fail loudly here rather than with an obscure bind error deep in a
        // test. Callers keep scratch space under /tmp for margin.
        assert!(
            socket_path.as_os_str().len() < 100,
            "tmux socket path too long for portability: {}",
            socket_path.display()
        );
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).expect("clear tmux socket path");
        }
        let listener = UnixListener::bind(&socket_path).expect("bind black-hole socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let (dialed_tx, dialed_rx) = mpsc::channel::<()>();
        let accepted: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let accepted = Arc::clone(&accepted);
            let stop = Arc::clone(&stop);
            let mut signaled = false;
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            accepted.lock().expect("lock streams").push(stream);
                            if !signaled {
                                signaled = true;
                                let _ = dialed_tx.send(());
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            stop,
            accepted,
            thread: Some(thread),
            dialed_rx,
        }
    }

    /// Blocks until a tmux client dials the socket, proving the stalled
    /// operation is in flight. Fails loudly on timeout so a fast-failing
    /// probe cannot pass as a stall.
    fn wait_for_dial(&self, timeout: Duration) {
        self.dialed_rx
            .recv_timeout(timeout)
            .expect("tmux client never dialed the black-hole socket");
    }

    /// Releases the stall: accepted clients fail, stalled workers proceed,
    /// and the accept thread exits.
    fn release(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join accept thread");
        }
        for stream in self.accepted.lock().expect("lock streams").drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

mod binding;
mod concurrency;
mod faults;
mod grammar;
mod updates;
