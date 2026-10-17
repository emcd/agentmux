use agentmux::{
    configuration::ConfigurationRoots,
    relay::{BundleCatalog, ConnectionDrainCoordinator, serve_connection},
    runtime::paths::BundleRuntimePaths,
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{hello_payload, read_json, send_json};

// Writes the outbound PSK for `alias` at `<state-root>/peers/<alias>.psk`, where
// the peer connection manager reads it on first forward.
pub(crate) fn write_peer_credential(
    state_root: &Path,
    alias: &str,
    psk: &str,
) {
    let peers_dir = state_root.join("peers");
    std::fs::create_dir_all(&peers_dir).expect("create peers dir");
    std::fs::write(peers_dir.join(format!("{alias}.psk")), psk)
        .expect("write peer psk");
}

// Serves one origin relay connection whose peer connection manager is configured
// with a single peer relay: `alias` is the bang-path `!<alias>` selector,
// `connect_as` is the identity this relay presents to the peer (`<connect_as>@RELAY`),
// and `peer_socket` is the peer's Unix socket. So a cross-relay Send is really
// dialed and forwarded rather than reported unavailable.
pub(crate) fn spawn_relay_stream_with_peer(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    alias: &str,
    connect_as: &str,
    peer_socket: &Path,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) =
        UnixStream::pair().expect("unix stream pair");
    let root = configuration_roots.clone();
    let state_root = bundle_paths.state_root.clone();
    let catalog = BundleCatalog::from_paths([bundle_paths.clone()]);
    let peers = vec![agentmux::relay::PeerConfiguration {
        alias: alias.to_string(),
        address: peer_socket.to_string_lossy().into_owned(),
        connect_as: connect_as.to_string(),
    }];
    let handle = thread::spawn(move || {
        run_serve_connection_with_peers(
            server_stream,
            root,
            state_root,
            catalog,
            peers,
        )
        .expect("serve connection");
    });
    (client_stream, handle)
}

pub(crate) fn run_serve_connection_with_peers(
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
            agentmux::relay::PeerConnectionManager::from_configuration(
                &state_root,
                &peers,
            ),
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
pub(crate) fn spawn_answering_peer(
    socket_path: &Path,
    response: Value,
) -> mpsc::Receiver<Value> {
    let listener =
        UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader =
            BufReader::new(stream.try_clone().expect("clone stub stream"));
        let mut stream = stream;
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let Ok(hello) = serde_json::from_str::<Value>(hello_line.trim_end())
        else {
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
            && let Ok(request) =
                serde_json::from_str::<Value>(request_line.trim_end())
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
pub(crate) fn ack_peer_hello(
    reader: &mut BufReader<UnixStream>,
    stream: &mut UnixStream,
) -> bool {
    let mut hello_line = String::new();
    if reader.read_line(&mut hello_line).is_err() {
        return false;
    }
    let Ok(hello) = serde_json::from_str::<Value>(hello_line.trim_end())
    else {
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
pub(crate) fn answer_one_peer_request(
    reader: &mut BufReader<UnixStream>,
    stream: &mut UnixStream,
    response: &Value,
) -> Option<Value> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let request =
        serde_json::from_str::<Value>(request_line.trim_end()).ok()?;
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
pub(crate) fn spawn_revoking_peer(
    socket_path: &Path,
    response: Value,
) -> (mpsc::Receiver<()>, mpsc::Receiver<Value>) {
    let listener =
        UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (closed_sender, closed_receiver) = mpsc::channel();
    let (request_sender, request_receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut first, _)) = listener.accept() else {
            return;
        };
        let mut first_reader =
            BufReader::new(first.try_clone().expect("clone stub stream"));
        if !ack_peer_hello(&mut first_reader, &mut first) {
            return;
        }
        if answer_one_peer_request(&mut first_reader, &mut first, &response)
            .is_none()
        {
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
        let mut second_reader =
            BufReader::new(second.try_clone().expect("clone stub stream"));
        if !ack_peer_hello(&mut second_reader, &mut second) {
            return;
        }
        if let Some(request) =
            answer_one_peer_request(&mut second_reader, &mut second, &response)
        {
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
pub(crate) struct ForwardedCrossRelaySend {
    pub(crate) response: Value,
    pub(crate) results: Vec<Value>,
    pub(crate) forwarded: Value,
}

// Hellos as socket-trust `alpha`, sends one cross-relay Send over the stream, and
// returns the decoded `send` response's `results` array plus the request frame
// the stub peer observed. Shared by the delivered/ingress-denied cases.
pub(crate) fn forward_cross_relay_send(
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
pub(crate) fn forward_cross_relay_raww(
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
pub(crate) fn forward_cross_relay_send_supplying_on_behalf_of(
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
pub(crate) fn forward_cross_relay_send_with_hello(
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
pub(crate) fn forward_cross_relay_send_inner(
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
