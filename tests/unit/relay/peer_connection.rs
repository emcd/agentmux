//! Outbound peer relay connection manager: PSK-bearing Hello handshake and
//! typed failure classification, exercised against an in-process stub peer.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agentmux::relay::{PeerConfiguration, PeerConnectionManager};
use serde_json::json;
use tempfile::TempDir;

/// Binds a one-shot stub peer relay that completes the Hello handshake and
/// reports the Hello frame it observed. The ack echoes the client's
/// `schema_version` and `principal_id`, so the test needs no knowledge of the
/// wire schema version. The listener is bound before returning so the manager
/// cannot dial ahead of it.
fn spawn_stub_peer(socket_path: &std::path::Path) -> mpsc::Receiver<serde_json::Value> {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stub stream"));
        let mut line = String::new();
        if reader.read_line(&mut line).is_ok()
            && let Ok(frame) = serde_json::from_str::<serde_json::Value>(line.trim_end())
        {
            let schema_version = frame.get("schema_version").cloned().unwrap_or(json!(""));
            let principal_id = frame.get("principal_id").cloned().unwrap_or(json!(""));
            let mut stream = stream;
            let ack = json!({
                "frame": "hello_ack",
                "schema_version": schema_version,
                "principal_id": principal_id,
            });
            let _ = writeln!(stream, "{ack}");
            let _ = stream.flush();
            let _ = sender.send(frame);
            // Hold the connection briefly so the client reads the ack before EOF.
            thread::sleep(Duration::from_millis(50));
        }
    });
    receiver
}

/// Binds a stub peer that completes the Hello handshake and then answers one
/// request, echoing the request frame's wire id in the response so the client's
/// per-request correlation matches. Returns the observed request frame.
fn spawn_answering_stub_peer(
    socket_path: &std::path::Path,
    response: serde_json::Value,
) -> mpsc::Receiver<serde_json::Value> {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stub stream"));
        let mut stream = stream;
        // Hello -> HelloAck (echo schema_version + principal_id).
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let Ok(hello) = serde_json::from_str::<serde_json::Value>(hello_line.trim_end()) else {
            return;
        };
        let ack = json!({
            "frame": "hello_ack",
            "schema_version": hello.get("schema_version").cloned().unwrap_or(json!("")),
            "principal_id": hello.get("principal_id").cloned().unwrap_or(json!("")),
        });
        let _ = writeln!(stream, "{ack}");
        let _ = stream.flush();
        // Request -> Response (echo the request's wire id for correlation).
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_ok()
            && let Ok(request) = serde_json::from_str::<serde_json::Value>(request_line.trim_end())
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

/// Binds a stub peer that serves `connection_total` Hello handshakes, reporting
/// each Hello frame, and half-closes every connection once its ack is written.
///
/// The half-close is what makes a redial deterministic. Shutting down the write
/// half delivers the ack and then EOF, which the client observes on its next
/// liveness peek and treats as a dead connection. Provoking the redial by writing
/// into a closed socket instead would depend on when the peer's close is noticed,
/// which is exactly the kind of timing this test must not rely on.
fn spawn_reconnecting_stub_peer(
    socket_path: &std::path::Path,
    connection_total: usize,
) -> mpsc::Receiver<serde_json::Value> {
    let listener = UnixListener::bind(socket_path).expect("bind stub peer socket");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..connection_total {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stub stream"));
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let Ok(hello) = serde_json::from_str::<serde_json::Value>(line.trim_end()) else {
                return;
            };
            let mut stream = stream;
            let ack = json!({
                "frame": "hello_ack",
                "schema_version": hello.get("schema_version").cloned().unwrap_or(json!("")),
                "principal_id": hello.get("principal_id").cloned().unwrap_or(json!("")),
            });
            let _ = writeln!(stream, "{ack}");
            let _ = stream.flush();
            // Ordering matters: the peer is already gone by the time the test
            // observes the frame, so the test never races the close.
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = sender.send(hello);
        }
    });
    receiver
}

fn write_peer_credential(state_root: &std::path::Path, alias: &str, psk: &str) {
    let peers_dir = state_root.join("peers");
    std::fs::create_dir_all(&peers_dir).expect("create peers dir");
    std::fs::write(peers_dir.join(format!("{alias}.psk")), psk).expect("write peer psk");
}

fn peer(alias: &str, connect_as: &str, address: &std::path::Path) -> PeerConfiguration {
    PeerConfiguration {
        alias: alias.to_string(),
        address: address.to_string_lossy().into_owned(),
        connect_as: connect_as.to_string(),
    }
}

#[test]
fn connect_completes_psk_hello_handshake_as_relay_principal() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    write_peer_credential(&state_root, "peer", "peer-secret-token");
    let socket_path: PathBuf = temporary.path().join("peer.sock");
    let observed = spawn_stub_peer(&socket_path);

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    assert!(manager.is_configured());
    manager.connect("peer").expect("connect to stub peer");

    let hello = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("stub observed the hello frame");
    assert_eq!(hello["frame"], "hello");
    assert_eq!(hello["principal_id"], "this-relay@RELAY");
    assert_eq!(hello["identity_token"], "peer-secret-token");
}

#[test]
fn connect_reports_missing_peer_credential() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    // No credential file and no listener: the credential check precedes the dial.
    let socket_path = temporary.path().join("peer.sock");

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    let error = manager
        .connect("peer")
        .expect_err("missing credential should fail the connection");
    assert_eq!(error.code, "runtime_peer_credential_missing");
}

#[test]
fn connect_rejects_unknown_peer() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    let socket_path = temporary.path().join("peer.sock");

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    let error = manager
        .connect("absent")
        .expect_err("an unconfigured peer id should be rejected");
    assert_eq!(error.code, "validation_unknown_peer");
}

#[test]
fn connect_reports_peer_unavailable_when_unreachable() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    write_peer_credential(&state_root, "peer", "peer-secret-token");
    // No listener bound at this path, so the dial fails fast (ENOENT).
    let socket_path = temporary.path().join("nonexistent.sock");

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    let error = manager
        .connect("peer")
        .expect_err("an unreachable peer should surface as unavailable");
    assert_eq!(error.code, "runtime_peer_unavailable");
}

#[test]
fn forward_reports_peer_unavailable_when_unreachable() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    write_peer_credential(&state_root, "peer", "peer-secret-token");
    // No listener bound: forwarding a request to an unreachable peer surfaces
    // the same typed unavailability outcome as a bare connect.
    let socket_path = temporary.path().join("nonexistent.sock");

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    let request = agentmux::relay::RelayRequest::Send {
        request_id: Some("origin-1".to_string()),
        requester_session: "this-relay@RELAY".to_string(),
        message: "hello".to_string(),
        targets: vec!["claude@myapp".to_string()],
        broadcast: false,
        on_behalf_of: None,
    };
    let error = manager
        .forward("peer", &request)
        .expect_err("forwarding to an unreachable peer should be unavailable");
    assert_eq!(error.code, "runtime_peer_unavailable");
}

#[test]
fn forward_returns_the_peer_response() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    write_peer_credential(&state_root, "peer", "peer-secret-token");
    let socket_path = temporary.path().join("peer.sock");
    let peer_response = json!({
        "kind": "raww",
        "schema_version": "test",
        "status": "queued",
        "target_session": "claude@myapp",
        "transport": "tmux",
        "request_id": "origin-1",
        "message_id": "m1",
    });
    let observed = spawn_answering_stub_peer(&socket_path, peer_response);

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    let request = agentmux::relay::RelayRequest::Raww {
        request_id: Some("origin-1".to_string()),
        requester_session: "this-relay@RELAY".to_string(),
        target_session: "claude@myapp".to_string(),
        text: "hello".to_string(),
        no_enter: false,
        on_behalf_of: None,
    };
    let response = manager
        .forward("peer", &request)
        .expect("forward returns the peer response");

    match response {
        agentmux::relay::RelayResponse::Raww {
            status,
            target_session,
            request_id,
            ..
        } => {
            assert_eq!(status, "queued");
            assert_eq!(target_session, "claude@myapp");
            assert_eq!(request_id.as_deref(), Some("origin-1"));
        }
        other => panic!("expected a forwarded raww response, got {other:?}"),
    }

    // The peer received a Raww request carrying this relay's identity and the
    // foreign target.
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("stub observed the forwarded request");
    assert_eq!(forwarded["request"]["operation"], "raww");
}

/// A credential rotated after the session was established is presented on the
/// next dial.
///
/// The manager holds one session per peer for the life of the process and reads
/// the credential fresh before every use. What made rotation unrecoverable was
/// that the fresh read only reached the session on the call that created it, so
/// every later dial re-presented the credential captured at construction — a
/// credential the peer had revoked. The operator-visible symptom was a peer stuck
/// on `Broken pipe` until the relay process was restarted.
#[test]
fn a_redial_presents_the_rotated_peer_credential() {
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("state");
    write_peer_credential(&state_root, "peer", "psk-original");
    let socket_path: PathBuf = temporary.path().join("peer.sock");
    let observed = spawn_reconnecting_stub_peer(&socket_path, 2);

    let manager = PeerConnectionManager::from_configuration(
        &state_root,
        &[peer("peer", "this-relay", &socket_path)],
    );
    manager.connect("peer").expect("establish the peer session");
    let first = observed
        .recv_timeout(Duration::from_secs(5))
        .expect("stub observed the first hello");
    assert_eq!(first["identity_token"], "psk-original");

    // Rotation, as `change psk` performs it: the file on disk changes while the
    // session that already authenticated under the old value stays cached.
    write_peer_credential(&state_root, "peer", "psk-rotated");

    // The peer half-closed above, so this redials rather than reusing the socket.
    manager
        .connect("peer")
        .expect("redial after the peer closed the connection");
    let second = observed
        .recv_timeout(Duration::from_secs(5))
        .expect("stub observed the redial hello");
    assert_eq!(
        second["identity_token"], "psk-rotated",
        "the redial must present the rotated credential, not the one captured when \
         the session was constructed"
    );
    assert_eq!(second["principal_id"], "this-relay@RELAY");
}

#[test]
fn presented_principal_id_qualifies_configured_connect_as() {
    let temporary = TempDir::new().expect("temporary");
    let socket = temporary.path().join("peer.sock");
    // The presented identity is per-peer: the alias `west` maps to the identity
    // `east` the peer issued this relay, qualified to `east@RELAY`.
    let manager = PeerConnectionManager::from_configuration(
        temporary.path(),
        &[peer("west", "east", &socket)],
    );
    assert_eq!(
        manager.presented_principal_id("west").as_deref(),
        Some("east@RELAY"),
    );
    assert!(manager.presented_principal_id("unknown-alias").is_none());
}

#[test]
fn manager_without_peers_is_not_configured() {
    let temporary = TempDir::new().expect("temporary");
    let manager = PeerConnectionManager::from_configuration(temporary.path(), &[]);
    assert!(!manager.is_configured());
}
