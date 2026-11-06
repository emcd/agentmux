//! Live-executable pipe tests for oversize inbound frames.
//!
//! The reference adapter discards an oversize frame at acquisition
//! without body-sized allocation and without answering it, and the
//! session continues: pre-negotiation discards keep waiting for the
//! hello result under the handshake deadline, post-negotiation
//! discards keep processing. Each integration file owns its fixture
//! helpers outright.

use std::io::Write;
use std::process::{Command, Stdio};

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    if path.file_name().and_then(|name| name.to_str()) == Some("deps") {
        path.pop();
    }
    path.join("agentmux-adapter")
}

fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("adapter.toml");
    std::fs::write(&path, script).expect("fixture writes");
    (dir, path)
}

const MINIMAL_SCRIPT: &str = r#"
[[turns]]
correlation = "c-1"
ack = "submitted"
"#;

fn hello_negotiated(
    max_frame: u64,
    drain_wait_ms: u64,
    tx: u64,
    rx: u64,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "result": {
            "runtime_instance": "rt-9",
            "adapter_generation": 3,
            "selected_major": 2,
            "selected_minor": 0,
            "snapshot_b64": "",
            "negotiated_limits": {
                "max_frame_bytes": max_frame,
                "queue_tx_bytes": tx,
                "queue_rx_bytes": rx,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": drain_wait_ms,
            },
        },
    })
}

/// The 64KiB bootstrap ceiling applies before negotiation: an
/// oversize hello result is discarded without body-sized allocation
/// and without answering it; with stdin at EOF the handshake then
/// fails for lack of a hello result (exit refusal, same as any
/// pre-binding EOF).
#[test]
fn oversize_hello_result_refused() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let mut hello = hello_negotiated(65536, 250, 16_777_216, 16_777_216);
    hello["result"]["snapshot_b64"] =
        serde_json::Value::String("x".repeat(70 * 1024));
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    let _ = stdin.write_all(&wire);
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("oversize_frame"),
        "the discard is recorded as a typed framing failure: {stderr}"
    );
    assert!(
        !stderr.contains("transport_unusable"),
        "no dead-transport terminalization for a discarded frame: {stderr}"
    );
}

/// An oversize post-hello frame is discarded and the session
/// continues: the next valid submit still terminalizes normally.
#[test]
fn oversize_frame_discarded_session_continues() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("adapter spawns");
    let hello = hello_negotiated(4096, 250, 16_777_216, 16_777_216);
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    // Oversize garbage (not even JSON): refused at acquisition
    // without allocation and without answering.
    wire.extend_from_slice(&vec![b'x'; 8192]);
    wire.push(b'\n');
    wire.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "10",
            "method": "adapter.submit",
            "params": {
                "protocol_major": 2,
                "protocol_minor": 0,
                "runtime_instance": "rt-9",
                "target_session": "seat-1",
                "adapter_generation": 3,
                "correlation_id": "c-1",
                "members": [{
                    "message_id": "m-1",
                    "target_session": "seat-1",
                    "adapter_generation": 3,
                    "content_type": "default",
                    "payload_b64": "aGk=",
                }],
            },
        }))
        .expect("serializes"),
    );
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("wire lands");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"outcome\":\"Submitted\""),
        "valid submit after the discarded frame still terminalizes: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("oversize_frame"),
        "the discard is recorded as a typed framing failure: {stderr}"
    );
}
