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

fn write_peer_credential(state_root: &std::path::Path, alias: &str, psk: &str) {
    let peers_dir = state_root.join("peers");
    std::fs::create_dir_all(&peers_dir).expect("create peers dir");
    std::fs::write(peers_dir.join(format!("{alias}.psk")), psk).expect("write peer psk");
}

fn peer(id: &str, address: &std::path::Path) -> PeerConfiguration {
    PeerConfiguration {
        id: id.to_string(),
        address: address.to_string_lossy().into_owned(),
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
        Some("this-relay".to_string()),
        &state_root,
        &[peer("peer@RELAY", &socket_path)],
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
        Some("this-relay".to_string()),
        &state_root,
        &[peer("peer@RELAY", &socket_path)],
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
        Some("this-relay".to_string()),
        &state_root,
        &[peer("peer@RELAY", &socket_path)],
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
        Some("this-relay".to_string()),
        &state_root,
        &[peer("peer@RELAY", &socket_path)],
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
        Some("this-relay".to_string()),
        &state_root,
        &[peer("peer@RELAY", &socket_path)],
    );
    let request = agentmux::relay::RelayRequest::Send {
        request_id: Some("origin-1".to_string()),
        requester_session: "this-relay@RELAY".to_string(),
        message: "hello".to_string(),
        targets: vec!["claude@myapp".to_string()],
        broadcast: false,
        quiet_window_ms: None,
    };
    let error = manager
        .forward("peer", &request)
        .expect_err("forwarding to an unreachable peer should be unavailable");
    assert_eq!(error.code, "runtime_peer_unavailable");
}

#[test]
fn own_relay_principal_id_qualifies_configured_relay_id() {
    let temporary = TempDir::new().expect("temporary");
    let manager = PeerConnectionManager::from_configuration(
        Some("this-relay".to_string()),
        temporary.path(),
        &[],
    );
    assert_eq!(
        manager.own_relay_principal_id().as_deref(),
        Some("this-relay@RELAY"),
    );

    let without = PeerConnectionManager::from_configuration(None, temporary.path(), &[]);
    assert!(without.own_relay_principal_id().is_none());
}

#[test]
fn manager_without_peers_is_not_configured() {
    let temporary = TempDir::new().expect("temporary");
    let manager = PeerConnectionManager::from_configuration(None, temporary.path(), &[]);
    assert!(!manager.is_configured());
}
