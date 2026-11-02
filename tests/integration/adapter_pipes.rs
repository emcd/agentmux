//! Live-executable pipe tests for the reference adapter binary.
//!
//! These drive the built binary over real OS pipes (not in-memory
//! buffers) to prove bounded behavior where it matters: a silent peer
//! cannot park the run loop past the handshake timeout, a partial
//! frame cannot either, a dead channel terminalizes instead of hanging,
//! and an outbox cap refusal terminalizes instead of queueing
//! unboundedly. Timeouts use real wall-clock bounds with generous
//! margins; no test asserts on timing precision, only on termination
//! and exit codes.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    if path.file_name().and_then(|name| name.to_str()) == Some("deps") {
        path.pop();
    }
    path.join("agentmux-adapter")
}

fn hello_result() -> serde_json::Value {
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
                "max_frame_bytes": 65536,
                "queue_tx_bytes": 16777216,
                "queue_rx_bytes": 16777216,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": 250,
            },
        },
    })
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

/// Polls for child exit, returning the code once observed.
/// Uses try_wait rather than wait: in this environment a plain wait on
/// a child with silent piped stdin returns immediately as if at EOF,
/// which would turn a silent peer into an EOF peer. Polling keeps the
/// pipe open so timeouts are actually exercised.
fn poll_exit(
    child: &mut std::process::Child,
    start: std::time::Instant,
    limit: Duration,
) -> Option<i32> {
    loop {
        if let Some(status) = child.try_wait().expect("wait works") {
            return status.code();
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A silent peer cannot park the run loop: the handshake timeout fires.
#[test]
fn silent_peer_hits_handshake_timeout() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let start = Instant::now();
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--handshake-timeout-ms",
            "200",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "must wait out the timeout, not fail instantly"
    );
    child.kill().ok();
}

/// A partial frame with no terminator cannot park the run loop either.
#[test]
fn partial_frame_hits_handshake_timeout() {
    let (_dir, script) = fixture(MINIMAL_SCRIPT);
    let start = Instant::now();
    let mut child = Command::new(binary())
        .args([
            "--target",
            "seat-1",
            "--script",
            script.to_str().expect("utf8"),
            "--handshake-timeout-ms",
            "200",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(b"{\"jsonrpc\": ")
        .expect("partial write lands");
    let code = poll_exit(&mut child, start, Duration::from_secs(10))
        .expect("adapter exits");
    assert_eq!(code, 2);
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "must wait out the timeout, not fail instantly"
    );
    child.kill().ok();
}

/// A closed channel terminalizes instead of hanging on write.
#[test]
fn closed_stdout_exits_unusable() {
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
        .stderr(Stdio::null())
        .spawn()
        .expect("adapter spawns");
    drop(child.stdout.take());
    let mut stdin = child.stdin.take().expect("stdin piped");
    let hello = serde_json::to_vec(&hello_result()).expect("serializes");
    stdin.write_all(&hello).expect("hello fits the pipe");
    stdin.write_all(b"\n").expect("newline lands");
    drop(stdin);
    let status = child.wait().expect("adapter exits");
    assert_eq!(status.code(), Some(2));
}

/// An outbox cap refusal terminalizes instead of queueing unboundedly.
#[test]
fn tiny_cap_refuses_instead_of_queueing() {
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
    let mut hello = hello_result();
    hello["result"]["negotiated_limits"]["queue_tx_bytes"] =
        serde_json::json!(10);
    let submit = serde_json::json!({
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
    });
    let mut wire = serde_json::to_vec(&hello).expect("serializes");
    wire.push(b'\n');
    let submit_bytes = serde_json::to_vec(&submit).expect("serializes");
    wire.extend_from_slice(&submit_bytes);
    wire.push(b'\n');
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(&wire).expect("fixtures fit the pipe");
    drop(stdin);
    let output = child.wait_with_output().expect("adapter exits");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("outbox_full"),
        "cap refusal must be diagnosed, got: {stderr}"
    );
}
