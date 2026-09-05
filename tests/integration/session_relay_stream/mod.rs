//! Integration tests for `serve_connection` end-to-end: stream-based UI
//! delivery, configured-session registration, choose authorization, and the
//! cross-relay (bang-path) send/ingress surface.
//!
//! The cluster files partition the 22 tests by concern:
//! - [`ui_delivery`]: connected UI stream event delivery (routing, reconnect
//!   hold, late-stream registration, async-sender outcome).
//! - [`registration`]: configured bundle member hello/registration.
//! - [`choices`]: choose-authorized list and pick rejection.
//! - [`cross_relay`]: outbound bang-path Send forwarding and ingress filter
//!   (peer-scope gate).
//! - [`on_behalf_of`]: cross-relay sender-attribution (`on_behalf_of`)
//!   stamping, surfacing, and spoof-gate behaviour.
//!
//! Shared helpers (every cluster shares the stream registry, the
//! per-bundle configuration writer, the per-test-unique `@GLOBAL` id, the
//! JSON line codec, and the ingress peer store fixtures) live in this
//! hub. Cluster-specific helpers live with their cluster.

use agentmux::configuration::ConfigurationRoots;
use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use agentmux::{
    relay::{
        BundleCatalog, ConnectionDrainCoordinator, RelayRequest, RelayResponse, handle_request,
        serve_connection,
    },
    runtime::paths::{BundleRuntimePaths, principal_store_path},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

mod choices;
mod cross_relay;
mod discovery;
mod on_behalf_of;
mod registration;
mod ui_delivery;

fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
}

fn write_bundle_configuration(temporary: &TempDir, bundle_name: &str) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders configuration");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"

# A relay-wide operator's home namespace is GLOBAL, so reaching into a bundle is
# cross-namespace and requires all.
[[policies]]
id = "operator"

[policies.controls]
list = "all"
look = "all"
send = "all"
"#,
    )
    .expect("write policies configuration");
    let global_id = global_user_id(bundle_name);
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            r#"
default-session = "{global_id}"

[[sessions]]
id = "{global_id}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration");
    ConfigurationRoots::single(configuration_root)
}

fn spawn_relay_stream(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_roots.clone();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let handle = thread::spawn(move || {
        run_serve_connection(server_stream, root, state_root, catalog).expect("serve connection");
    });
    (client_stream, handle)
}

// Bridges `serve_connection` (now async) into a synchronous test thread by
// owning a dedicated current-thread tokio runtime per connection.
fn run_serve_connection(
    server_stream: UnixStream,
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
) -> Result<(), std::io::Error> {
    server_stream
        .set_nonblocking(true)
        .expect("non-blocking server stream");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");
    runtime.block_on(async move {
        let stream = tokio::net::UnixStream::from_std(server_stream)?;
        // No peers configured for these tests: an empty manager never dials.
        let peer_connection_manager = std::sync::Arc::new(
            agentmux::relay::PeerConnectionManager::from_configuration(&state_root, &[]),
        );
        let serve_context = agentmux::relay::ConnectionServeContext::new(
            configuration_roots,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            Vec::new(),
            false,
            Duration::from_secs(2),
        );
        serve_connection(
            stream,
            &serve_context,
            ConnectionDrainCoordinator::new().register_worker(),
        )
        .await
    })
}

fn send_json(stream: &mut UnixStream, payload: Value) {
    let encoded = serde_json::to_string(&payload).expect("encode payload");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write payload");
    stream.flush().expect("flush payload");
}

fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("read payload");
    assert!(read > 0, "expected payload");
    serde_json::from_str::<Value>(line.trim_end()).expect("decode payload")
}

fn read_json_with_timeout(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(read) => {
            if read == 0 {
                return None;
            }
            Some(serde_json::from_str::<Value>(line.trim_end()).expect("decode payload"))
        }
        Err(source) if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => None,
        Err(source) => panic!("read payload: {source}"),
    }
}

// Derives a short, unique `@GLOBAL` operator id from a (per-test unique) bundle
// name. Relay-wide principals are keyed in the process-wide stream registry by
// `principal_id` alone, so concurrent tests must not share one global id.
fn global_user_id(bundle_name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bundle_name.hash(&mut hasher);
    format!("g{:016x}@GLOBAL", hasher.finish())
}

// Collects stream events addressed to `target_session` until a terminal
// `delivered` outcome is seen or the deadline elapses. `terminal_message_id`
// selects which delivery ends collection: `Some(id)` stops only on the
// `delivered` outcome of that specific message, while `None` stops on the first
// `delivered` for any message. A specific id is required whenever an earlier
// delivery's outcome can reach the stream first — e.g. a message held before the
// UI stream existed is flushed (with its own `delivered` outcome) the moment a
// late stream registers, ahead of a later send's events. Passing `None` there
// would break collection on the held message and miss the later one. Relay-wide
// (`@GLOBAL`) UI connections receive events from every bundle on the relay, and
// the stream registry is process-wide, so a test must filter foreign events out
// by the recipient id in the canonical `target_session` (unique per test) rather
// than reading a fixed event count.
fn collect_events_for_target(
    stream: &UnixStream,
    reader: &mut BufReader<UnixStream>,
    target_session: &str,
    terminal_message_id: Option<&str>,
    deadline: Duration,
) -> Vec<Value> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set read timeout");
    let end = std::time::Instant::now() + deadline;
    let mut events = Vec::new();
    while std::time::Instant::now() < end {
        let Some(value) = read_json_with_timeout(reader) else {
            continue;
        };
        if value["frame"] != "event" || value["event"]["target_session"] != target_session {
            continue;
        }
        let terminal = value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "delivered"
            && terminal_message_id
                .is_none_or(|id| value["event"]["payload"]["message_id"].as_str() == Some(id));
        events.push(value);
        if terminal {
            break;
        }
    }
    let _ = stream.set_read_timeout(None);
    events
}

fn hello_payload(bundle_name: &str, session_id: &str) -> Value {
    let principal_id = if session_id.ends_with("@GLOBAL") {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    };
    json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": principal_id,
        "identity_token": "socket-trust",
    })
}

// Writes the standard configuration plus a configured (non-`@GLOBAL`) `display`
// UI member, so a send can target a configured UI session whose UI stream may
// register after the first delivery.
fn write_bundle_configuration_with_ui_member(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_roots = write_bundle_configuration(temporary, bundle_name);
    let bundles_directory = configuration_roots.base_layer().join("bundles");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"

[[sessions]]
id = "display"
name = "Display"
directory = "/tmp"

[sessions.ui]
"#,
    )
    .expect("rewrite bundle configuration with ui member");
    configuration_roots
}

fn write_operator_bundle_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders configuration");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"
choose = "none"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"
choose = "home"
"#,
    )
    .expect("write policies configuration");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
policy = "operator"
coder = "shell"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration");
    ConfigurationRoots::single(configuration_root)
}

// Bundle whose member `alpha` holds `send = all`, so it may address a cross-relay
// (always all-tier) target. Otherwise a minimal single-member tmux bundle.
fn write_cross_relay_bundle_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_root = temporary.path().join("config");
    let bundles_directory = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles_directory).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders configuration");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "peer_sender"

[[policies]]
id = "peer_sender"

[policies.controls]
list = "home"
look = "self"
send = "all"
raww = "all"
"#,
    )
    .expect("write policies configuration");
    std::fs::write(
        bundles_directory.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration");
    ConfigurationRoots::single(configuration_root)
}

// Writes the outbound PSK for `alias` at `<state-root>/peers/<alias>.psk`, where
// the peer connection manager reads it on first forward.
fn write_peer_credential(state_root: &Path, alias: &str, psk: &str) {
    let peers_dir = state_root.join("peers");
    std::fs::create_dir_all(&peers_dir).expect("create peers dir");
    std::fs::write(peers_dir.join(format!("{alias}.psk")), psk).expect("write peer psk");
}

// Serves one origin relay connection whose peer connection manager is configured
// with a single peer relay: `alias` is the bang-path `!<alias>` selector,
// `connect_as` is the identity this relay presents to the peer (`<connect_as>@RELAY`),
// and `peer_socket` is the peer's Unix socket. So a cross-relay Send is really
// dialed and forwarded rather than reported unavailable.
fn spawn_relay_stream_with_peer(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    alias: &str,
    connect_as: &str,
    peer_socket: &Path,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_roots.clone();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let peers = vec![agentmux::relay::PeerConfiguration {
        alias: alias.to_string(),
        address: peer_socket.to_string_lossy().into_owned(),
        connect_as: connect_as.to_string(),
    }];
    let handle = thread::spawn(move || {
        run_serve_connection_with_peers(server_stream, root, state_root, catalog, peers)
            .expect("serve connection");
    });
    (client_stream, handle)
}

fn run_serve_connection_with_peers(
    server_stream: UnixStream,
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
    peers: Vec<agentmux::relay::PeerConfiguration>,
) -> Result<(), std::io::Error> {
    server_stream
        .set_nonblocking(true)
        .expect("non-blocking server stream");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");
    runtime.block_on(async move {
        let stream = tokio::net::UnixStream::from_std(server_stream)?;
        let peer_connection_manager = std::sync::Arc::new(
            agentmux::relay::PeerConnectionManager::from_configuration(&state_root, &peers),
        );
        let serve_context = agentmux::relay::ConnectionServeContext::new(
            configuration_roots,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            peers.iter().map(|peer| peer.alias.clone()).collect(),
            false,
            Duration::from_secs(2),
        );
        serve_connection(
            stream,
            &serve_context,
            ConnectionDrainCoordinator::new().register_worker(),
        )
        .await
    })
}

// A one-shot stub peer relay: completes the PSK Hello handshake (echoing the
// dialer's schema_version + principal_id) and answers the single forwarded
// request with `response`, echoing the wire request id for correlation. Returns
// the request frame it observed.
fn spawn_answering_peer(socket_path: &Path, response: Value) -> mpsc::Receiver<Value> {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stub stream"));
        let mut stream = stream;
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let Ok(hello) = serde_json::from_str::<Value>(hello_line.trim_end()) else {
            return;
        };
        let ack = json!({
            "frame": "hello_ack",
            "schema_version": hello.get("schema_version").cloned().unwrap_or(json!("")),
            "principal_id": hello.get("principal_id").cloned().unwrap_or(json!("")),
        });
        let _ = writeln!(stream, "{ack}");
        let _ = stream.flush();
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_ok()
            && let Ok(request) = serde_json::from_str::<Value>(request_line.trim_end())
        {
            let response_frame = json!({
                "frame": "response",
                "request_id": request.get("request_id").cloned().unwrap_or(json!(null)),
                "response": response,
            });
            let _ = writeln!(stream, "{response_frame}");
            let _ = stream.flush();
            let _ = sender.send(request);
            thread::sleep(Duration::from_millis(50));
        }
    });
    receiver
}

// Completes a stub peer's PSK Hello handshake, echoing the dialer's
// schema_version and principal_id. Returns whether the ack was written.
fn ack_peer_hello(reader: &mut BufReader<UnixStream>, stream: &mut UnixStream) -> bool {
    let mut hello_line = String::new();
    if reader.read_line(&mut hello_line).is_err() {
        return false;
    }
    let Ok(hello) = serde_json::from_str::<Value>(hello_line.trim_end()) else {
        return false;
    };
    let ack = json!({
        "frame": "hello_ack",
        "schema_version": hello.get("schema_version").cloned().unwrap_or(json!("")),
        "principal_id": hello.get("principal_id").cloned().unwrap_or(json!("")),
    });
    let _ = writeln!(stream, "{ack}");
    stream.flush().is_ok()
}

// Answers one forwarded request with `response`, echoing the wire request id
// for correlation. Returns the request frame observed.
fn answer_one_peer_request(
    reader: &mut BufReader<UnixStream>,
    stream: &mut UnixStream,
    response: &Value,
) -> Option<Value> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let request = serde_json::from_str::<Value>(request_line.trim_end()).ok()?;
    let response_frame = json!({
        "frame": "response",
        "request_id": request.get("request_id").cloned().unwrap_or(json!(null)),
        "response": response,
    });
    let _ = writeln!(stream, "{response_frame}");
    let _ = stream.flush();
    Some(request)
}

// A stub peer relay that revokes the dialer mid-life: it answers one forwarded
// request, then writes a typed error frame and closes -- the teardown order a
// `drop peer` produces -- and afterwards accepts a fresh dial and answers one
// more request.
//
// Returns a channel signalling that the first connection is fully closed, so a
// test sequences on the close rather than on a sleep, and a channel carrying
// the request frame observed on the second connection.
fn spawn_revoking_peer(
    socket_path: &Path,
    response: Value,
) -> (mpsc::Receiver<()>, mpsc::Receiver<Value>) {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (closed_sender, closed_receiver) = mpsc::channel();
    let (request_sender, request_receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut first, _)) = listener.accept() else {
            return;
        };
        let mut first_reader = BufReader::new(first.try_clone().expect("clone stub stream"));
        if !ack_peer_hello(&mut first_reader, &mut first) {
            return;
        }
        if answer_one_peer_request(&mut first_reader, &mut first, &response).is_none() {
            return;
        }
        let revoked = json!({
            "frame": "response",
            "response": {
                "kind": "error",
                "error": {
                    "code": "runtime_identity_revoked",
                    "message": "identity was dropped; the credential no longer authenticates",
                },
            },
        });
        let _ = writeln!(first, "{revoked}");
        let _ = first.flush();
        // Close every handle. A surviving clone would keep this end of the
        // socket open, and the frame above would then be an ordinary unread
        // event rather than the dialer's last word before a hangup.
        drop(first_reader);
        drop(first);
        let _ = closed_sender.send(());

        let Ok((mut second, _)) = listener.accept() else {
            return;
        };
        let mut second_reader = BufReader::new(second.try_clone().expect("clone stub stream"));
        if !ack_peer_hello(&mut second_reader, &mut second) {
            return;
        }
        if let Some(request) = answer_one_peer_request(&mut second_reader, &mut second, &response) {
            let _ = request_sender.send(request);
            thread::sleep(Duration::from_millis(50));
        }
    });
    (closed_receiver, request_receiver)
}

// What one forwarded cross-relay Send produced, at both points a test may need
// to observe. The origin's own response and the request the peer received are
// separate surfaces carrying separately sourced attribution — the response says
// whether a credential backed the sender, the forwarded request says who the
// sender was admitted as — so a test pinning that distinction needs both.
struct ForwardedCrossRelaySend {
    response: Value,
    results: Vec<Value>,
    forwarded: Value,
}

// Hellos as socket-trust `alpha`, sends one cross-relay Send over the stream, and
// returns the decoded `send` response's `results` array plus the request frame
// the stub peer observed. Shared by the delivered/ingress-denied cases.
fn forward_cross_relay_send(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
) -> ForwardedCrossRelaySend {
    forward_cross_relay_send_with_hello(
        configuration_roots,
        bundle_paths,
        peer_socket,
        observed,
        hello_payload(bundle_name, "alpha"),
    )
}

// Hellos as socket-trust `alpha` and forwards one cross-relay Raww, returning
// the request frame the stub peer observed. Raww's cross-relay branch runs
// before the local delivery spine and stamps its own attribution, so it needs
// coverage of its own rather than riding on the Send path's.
fn forward_cross_relay_raww(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
) -> Value {
    let (mut client, handle) = spawn_relay_stream_with_peer(
        configuration_roots,
        bundle_paths,
        "peer",
        "origin-relay",
        peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name, "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "raww",
                "requester_session": "alpha",
                "target_session": "bravo@other!peer",
                "text": "cross-relay keystrokes",
                "no_enter": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(
        response["response"]["kind"], "raww",
        "cross-relay raww should forward, not error: {response}"
    );
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("stub peer observed the forwarded raww");

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    forwarded
}

// As `forward_cross_relay_send`, but the request carries a self-asserted
// `on_behalf_of`. Returns only the observed peer request, which is where the
// relay's substituted attribution is visible.
fn forward_cross_relay_send_supplying_on_behalf_of(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
    supplied_on_behalf_of: &str,
) -> Value {
    forward_cross_relay_send_inner(
        configuration_roots,
        bundle_paths,
        peer_socket,
        observed,
        hello_payload(bundle_name, "alpha"),
        Some(supplied_on_behalf_of),
    )
    .forwarded
}

// As `forward_cross_relay_send`, but the requester authenticates with the given
// hello frame. Lets a test drive a store-backed origin alongside the default
// socket-trust one; both carry an `on_behalf_of` attribution, and the two differ
// in whether `authenticated_identity` accompanies it.
fn forward_cross_relay_send_with_hello(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
    requester_hello: Value,
) -> ForwardedCrossRelaySend {
    forward_cross_relay_send_inner(
        configuration_roots,
        bundle_paths,
        peer_socket,
        observed,
        requester_hello,
        None,
    )
}

// The shared body. `supplied_on_behalf_of` puts a self-asserted origin on the
// request, which the relay is expected to discard rather than honor.
fn forward_cross_relay_send_inner(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    peer_socket: &Path,
    observed: mpsc::Receiver<Value>,
    requester_hello: Value,
    supplied_on_behalf_of: Option<&str>,
) -> ForwardedCrossRelaySend {
    let (mut client, handle) = spawn_relay_stream_with_peer(
        configuration_roots,
        bundle_paths,
        "peer",
        "origin-relay",
        peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, requester_hello);
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    let mut request = json!({
        "operation": "send",
        "requester_session": "alpha",
        "message": "cross-relay hello",
        "targets": ["bravo@other!peer"],
        "broadcast": false,
    });
    if let Some(origin) = supplied_on_behalf_of {
        request["on_behalf_of"] = Value::String(origin.to_string());
    }
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": request,
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array")
        .clone();
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("stub peer observed the forwarded send");

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    ForwardedCrossRelaySend {
        response,
        results,
        forwarded,
    }
}

// SHA-256 (lowercase hex) of the fixed ingress PSK below, embedded in the
// hand-written principal store so a Hello with the raw token authenticates.
const INGRESS_PEER_TOKEN: &str = "ingress-peer-secret";
const INGRESS_PEER_CREDENTIAL_HASH: &str =
    "1c9c2d8823d0f52409743bb29168008f69157c166de636fcca23631e26f8daa7";

/// Negative-assertion budget: how long to wait after `SendOutcome::Queued`
/// for the worker to attempt delivery against a not-yet-registered UI
/// stream, before the test registers the UI stream. The worker must
/// have had time to subscribe-fail-drop for the routing decision to be
/// exercised; 300ms is generous for the worker cycle on any loaded
/// CI machine. Reduce with a per-test override if a future test
/// shows the cycle is faster.
const UI_DELIVERY_WORKER_BUDGET: Duration = Duration::from_millis(300);

// A per-test-unique peer relay principal id. The process-wide stream registry is
// keyed by `principal_id`, so concurrent tests must not share one or they collide
// with an identity-claim conflict. The credential hash is fixed (tied to the one
// token), independent of the id.
fn unique_relay_principal_id() -> String {
    format!("origin-{}@RELAY", Uuid::new_v4().simple())
}

// Hand-writes a principal store registering `principal_id` as a peer relay with
// the fixed ingress credential and the given `scope` (`None` for a peer
// registered without a scope, which the ingress gate treats as fail-closed).
fn write_ingress_peer_store(state_root: &Path, principal_id: &str, scope: Option<&str>) {
    let store_path = principal_store_path(state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let scope_field = match scope {
        Some(value) => format!(",\n      \"scope\": \"{value}\""),
        None => String::new(),
    };
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"relay\",\n      \"credential_hash\": \"{INGRESS_PEER_CREDENTIAL_HASH}\"{scope_field}\n    }}\n  ]\n}}"
    );
    std::fs::write(&store_path, body).expect("write principal store");
}

// Hand-writes a principal store registering `principal_id` as a store-backed
// (PSK-authenticated) session principal with the fixed ingress credential, so a
// Hello with `INGRESS_PEER_TOKEN` yields a verified `authenticated_identity`
// (unlike a socket-trust session, which carries none).
fn write_authenticated_session_store(state_root: &Path, principal_id: &str) {
    let store_path = principal_store_path(state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"session\",\n      \"credential_hash\": \"{INGRESS_PEER_CREDENTIAL_HASH}\"\n    }}\n  ]\n}}"
    );
    std::fs::write(&store_path, body).expect("write principal store");
}

// Hellos as `principal_id` (a peer relay) with its registered PSK, submits one
// request, and returns the decoded response frame.
fn ingress_request_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    request: Value,
) -> Value {
    let (mut client, handle) = spawn_relay_stream(configuration_roots, bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": INGRESS_PEER_TOKEN,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": request,
        }),
    );
    let response = read_json(&mut reader);
    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    response
}

// Convenience wrapper: forwards an ingress Send to a single `target`.
fn ingress_send_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    target: &str,
) -> Value {
    ingress_request_response(
        configuration_roots,
        bundle_paths,
        principal_id,
        json!({
            "operation": "send",
            "requester_session": principal_id,
            "message": "ingress hello",
            "targets": [target],
            "broadcast": false,
        }),
    )
}
